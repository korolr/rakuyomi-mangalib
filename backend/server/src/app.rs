use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
#[cfg(not(feature = "ffi"))]
use std::sync::mpsc;
use std::sync::Arc;

use anyhow::{Context, Result};
use log::{info, warn, LevelFilter, Log, Metadata, Record};
use tokio::sync::{Mutex, Semaphore};

#[cfg(feature = "ffi")]
use axum::extract::Request;
#[cfg(feature = "ffi")]
use axum::middleware::{self, Next};
#[cfg(feature = "ffi")]
use axum::response::Response;
use axum::{routing::get, Json, Router};
use shared::chapter_storage::ChapterStorage;
use shared::database::Database;
use shared::settings::Settings;
use shared::source_manager::SourceManager;
use shared::usecases::install_update::cleanup_update_backup;

use crate::build_info::{get_build_info, DEFAULT_SETTINGS_JSON};
use crate::listener::{pick_listener, ResolvedListener};
use crate::state::State;
use crate::{cookie, job, manga, playlists, settings, source, system, update};

/// Initialize logging. Safe to call multiple times; only the first invocation
/// installs the logger.
///
/// - **Linux e-readers** (`not(ffi)`): Bounded `sync_channel(200)` with a
///   background thread. `try_send()` drops messages when full — zero blocking,
///   zero OOM risk on low-RAM devices.
/// - **Android** (`ffi`): Direct stderr writes — JNI captures stderr natively,
///   no channel or background thread needed.
pub fn init_logging() {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    let max_level = parse_log_level(&std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()));

    #[cfg(not(feature = "ffi"))]
    {
        let (sender, receiver) = mpsc::sync_channel::<String>(200);

        std::thread::Builder::new()
            .name("rakuyomi-log".into())
            .spawn(move || {
                let mut stderr = std::io::stderr();
                while let Ok(msg) = receiver.recv() {
                    let _ = stderr.write_all(msg.as_bytes());
                }
            })
            .expect("failed to spawn logging thread");

        let _ = log::set_boxed_logger(Box::new(ChannelLogger { sender }));
    }

    #[cfg(feature = "ffi")]
    {
        let _ = log::set_boxed_logger(Box::new(StderrLogger));
    }

    log::set_max_level(max_level);
}

fn parse_log_level(rust_log: &str) -> LevelFilter {
    // Parse all comma-separated directives like "info,server=debug,shared=trace"
    // and return the most verbose (highest) level found.
    let mut max_level = LevelFilter::Info;
    for directive in rust_log.split(',') {
        // Strip target prefix: "server=debug" → "debug"
        let level_str = directive
            .trim()
            .rsplit_once('=')
            .map(|(_, level)| level)
            .unwrap_or(directive.trim());
        let level = match level_str.to_lowercase().as_str() {
            "error" => LevelFilter::Error,
            "warn" | "warning" => LevelFilter::Warn,
            "info" => LevelFilter::Info,
            "debug" => LevelFilter::Debug,
            "trace" => LevelFilter::Trace,
            _ => continue,
        };
        if level > max_level {
            max_level = level;
        }
    }
    max_level
}

/// A `log::Log` implementation that sends formatted messages through a bounded
/// channel. When the channel is full, messages are silently dropped.
#[cfg(not(feature = "ffi"))]
struct ChannelLogger {
    sender: mpsc::SyncSender<String>,
}

#[cfg(not(feature = "ffi"))]
impl Log for ChannelLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // Compact format: [LEVEL target] message
        // No timestamps — the Lua log capturer adds its own context.
        let msg = format!("[{}] {}\n", record.level(), record.args(),);

        // Silently drop if the channel is full (bounded at 200).
        let _ = self.sender.try_send(msg);
    }

    fn flush(&self) {}
}

/// Android-only: writes directly to stderr. JNI captures stderr natively,
/// so no channel or background thread is needed.
#[cfg(feature = "ffi")]
struct StderrLogger;

#[cfg(feature = "ffi")]
impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "[{}] {}", record.level(), record.args());
    }

    fn flush(&self) {}
}

/// Build the full axum router with the given state. Exposed so both the
/// binary entry point and the JNI bridge can share it.
pub fn build_router(state: State) -> Router {
    let router = Router::new()
        .route("/health-check", get(health_check))
        .merge(cookie::routes())
        .merge(manga::routes())
        .merge(playlists::routes())
        .merge(job::routes())
        .merge(settings::routes())
        .merge(source::routes())
        .merge(update::routes())
        .merge(system::routes());
    #[cfg(feature = "ffi")]
    let router = router
        .layer(middleware::from_fn(request_logger))
        .with_state(state);
    #[cfg(not(feature = "ffi"))]
    let router = router.with_state(state);
    router
}

#[cfg(feature = "ffi")]
async fn request_logger(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();

    let response = next.run(req).await;

    let duration = start.elapsed().as_millis();
    let status = response.status().as_u16();

    #[cfg(all(target_os = "android", feature = "ffi"))]
    {
        // Format as CSV-like string for Kotlin to parse easily
        // method|path|status|duration
        let log_entry = format!("{}|{}|{}|{}", method, path, status, duration);
        crate::jni::push_log(log_entry);
    }

    response
}

async fn health_check() -> Json<()> {
    Json(())
}

/// Construct the [`State`] from a home path. This performs all the
/// filesystem bootstrapping (creating directories, writing default
/// settings, opening the database) and source loading.
pub async fn build_state(home_path: PathBuf) -> Result<State> {
    fs::create_dir_all(&home_path)
        .context("while trying to ensure rakuyomi's home folder exists")?;

    let sources_path = home_path.join("sources");
    let database_path = home_path.join("database.db");
    let default_downloads_folder_path = home_path.join("downloads");
    let settings_path = home_path.join("settings.json");

    let database = Database::new(&database_path)
        .await
        .context("couldn't open database file")?;
    if !settings_path.exists() {
        info!(
            "settings file not found at {}, creating default",
            settings_path.display()
        );
        fs::write(&settings_path, DEFAULT_SETTINGS_JSON).with_context(|| {
            format!(
                "couldn't write default settings file at {}",
                settings_path.display()
            )
        })?;
    }
    let cookies_path = home_path.join("cookies.json");
    shared::cookie_store::init_cookie_store_with_path(&cookies_path)
        .context("couldn't initialize cookie store")?;
    let mut settings = Settings::from_file(&settings_path)
        .with_context(|| format!("couldn't read settings file at {}", settings_path.display()))?;
    import_source_settings(&home_path, &settings_path, &mut settings);
    if shared::settings::linked::fill_missing(&mut settings) {
        if let Err(e) = settings.save_to_file(&settings_path) {
            warn!("couldn't save linked source settings: {e:#}");
        }
    }

    shared::tls::set_proxy_url(settings.proxy_url.clone());

    let source_manager = SourceManager::from_folder(sources_path, settings.clone())
        .context("couldn't create source manager")?;

    let downloads_folder_path = settings
        .storage_path
        .clone()
        .unwrap_or(default_downloads_folder_path);

    let startup_log = crate::state::StartupLog::new();

    let mut chapter_storage = ChapterStorage::new(
        downloads_folder_path,
        settings.storage_size_limit.0,
        settings.ram_storage_enabled,
    )
    .context("couldn't initialize chapter storage")?;

    if settings.ram_storage_enabled {
        // Clean up old files on startup
        let _ = chapter_storage.clean_tmpfs().await;
        if let Err(e) = chapter_storage.enable_ram(settings.ram_storage_size_mb) {
            let hint = crate::error::setcap_hint();
            let msg = format!("RAM storage unavailable at startup: {e:#}{hint}");
            warn!("{msg}");
            startup_log.push(msg).await;
        }
    }

    if settings.enabled_cron_check_mangas_update {
        let db_clone = database.clone();
        let sm_clone = source_manager.clone();
        let cs_clone = chapter_storage.clone();
        let settings = settings.clone();

        tokio::spawn(async move {
            shared::usecases::run_manga_cron(&db_clone, &cs_clone, &sm_clone, &settings).await;
        });
    }

    let state = State {
        source_manager: Arc::new(Mutex::new(source_manager)),
        database: Arc::new(database),
        chapter_storage: Arc::new(Mutex::new(chapter_storage)),
        settings: Arc::new(Mutex::new(settings)),
        settings_path,
        job_state: Default::default(),
        cancel_token_store: Arc::new(Mutex::new(HashMap::new())),
        download_semaphore: Arc::new(Semaphore::new(3)),
        startup_log,
    };

    {
        let mut source_manager = state.source_manager.lock().await;
        source_manager.sources_by_id = source_manager
            .load_all_sources(&state.source_manager)
            .context("couldn't load sources")?;
    }

    spawn_defaults_persister(&state);

    Ok(state)
}

/// Imports per-source settings dropped by the user into `<home>/import/`.
///
/// Each `<source_id>.json` file must contain a JSON object; its entries are
/// merged into the stored settings of that source (non-string scalars keep
/// their type, nested objects are stored as JSON strings). The file is deleted
/// afterwards, as it typically contains secrets (e.g. MangaLib tokens) that
/// would otherwise override newer, refreshed values on every start.
fn import_source_settings(
    home_path: &std::path::Path,
    settings_path: &std::path::Path,
    settings: &mut Settings,
) {
    use shared::settings::SourceSettingValue;

    let import_dir = home_path.join("import");
    let Ok(entries) = fs::read_dir(&import_dir) else {
        return;
    };
    let mut changed = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(source_id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let parsed = fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|raw| {
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                    raw.trim_start_matches('\u{feff}'),
                )
                .map_err(anyhow::Error::from)
            });
        let object = match parsed {
            Ok(object) => object,
            Err(e) => {
                warn!("couldn't import settings from {}: {e:#}", path.display());
                continue;
            }
        };
        let object = shared::settings::linked::normalize_import(&source_id, object);
        let previous = settings.source_settings.get(&source_id).cloned();
        let stored = settings
            .source_settings
            .entry(source_id.clone())
            .or_default();
        for (key, value) in object {
            let value = match value {
                serde_json::Value::Null => SourceSettingValue::Null,
                serde_json::Value::Bool(b) => SourceSettingValue::Bool(b),
                serde_json::Value::Number(n) => match n.as_i64() {
                    Some(i) => SourceSettingValue::Int(i),
                    None => SourceSettingValue::Float(n.as_f64().unwrap_or_default()),
                },
                serde_json::Value::String(s) => SourceSettingValue::String(s),
                serde_json::Value::Array(items) if items.iter().all(|i| i.is_string()) => {
                    SourceSettingValue::Vec(
                        items
                            .into_iter()
                            .filter_map(|i| i.as_str().map(str::to_owned))
                            .collect(),
                    )
                }
                other => SourceSettingValue::String(other.to_string()),
            };
            stored.insert(key, value);
        }
        shared::settings::linked::clear_live(&source_id);
        shared::settings::linked::propagate_changes(settings, &source_id, previous.as_ref());
        changed = true;
        info!(
            "imported settings for source {source_id} from {}",
            path.display()
        );
        if let Err(e) = fs::remove_file(&path) {
            warn!(
                "couldn't remove imported settings file {}: {e:#}",
                path.display()
            );
        }
    }
    if changed {
        if let Err(e) = settings.save_to_file(settings_path) {
            warn!("couldn't save imported source settings: {e:#}");
        }
    }
}

/// Persists values written by sources through `defaults.set` (for example a
/// refreshed access/refresh token pair) into `settings.json`, so they survive
/// restarts. Runs outside of the WASM call to avoid lock re-entrancy.
fn spawn_defaults_persister(state: &State) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(
        String,
        String,
        shared::settings::SourceSettingValue,
    )>();
    shared::source::source_settings::set_defaults_persister(Box::new(
        move |source_id, key, value| {
            let _ = tx.send((source_id, key, value));
        },
    ));

    let settings = state.settings.clone();
    let settings_path = state.settings_path.clone();
    let source_manager = state.source_manager.clone();
    tokio::spawn(async move {
        while let Some((source_id, key, value)) = rx.recv().await {
            let mut settings = settings.lock().await;
            let mut updated = settings.clone();
            updated
                .source_settings
                .entry(source_id.clone())
                .or_default()
                .insert(key.clone(), value.clone());
            shared::settings::linked::propagate_key(&mut updated, &source_id, &key, &value);
            if let Err(e) = updated.save_to_file(&settings_path) {
                warn!("failed to persist source setting {source_id}/{key}: {e:#}");
                continue;
            }
            let mut manager = source_manager.lock().await;
            if let Err(e) = manager.update_settings(updated.clone(), &source_manager) {
                warn!("failed to apply source setting {source_id}/{key}: {e:#}");
            }
            *settings = updated;
            info!("persisted source setting {source_id}/{key}");
        }
    });
}

/// Run the server on a transport chosen by [`pick_listener`].
///
/// This is the main entry point used by the standalone binary. Blocks
/// until the listener is closed or an error occurs.
pub async fn run(home_path: PathBuf) -> Result<()> {
    cleanup_update_backup();
    let listener = pick_listener().await?;
    let state = build_state(home_path).await?;
    let version = get_build_info()
        .map(|info| info.format_display())
        .unwrap_or_else(|| "unknown".into());
    info!("starting rakuyomi {} on {}", version, listener.describe());
    serve(listener, state, None).await
}

/// Serve the router on the given listener, optionally listening for a
/// shutdown signal. Returns when the server stops (either due to an
/// error or the shutdown signal firing).
pub async fn serve(
    listener: ResolvedListener,
    state: State,
    shutdown: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<()> {
    let app = build_router(state);
    let shutdown_fut = shutdown.map(|rx| async move {
        let _ = rx.await;
    });
    match (listener, shutdown_fut) {
        (ResolvedListener::Tcp(l), Some(s)) => {
            axum::serve(l, app).with_graceful_shutdown(s).await?
        }
        (ResolvedListener::Tcp(l), None) => axum::serve(l, app).await?,
        (ResolvedListener::Unix(l, _), Some(s)) => {
            axum::serve(l, app).with_graceful_shutdown(s).await?
        }
        (ResolvedListener::Unix(l, _), None) => axum::serve(l, app).await?,
    }
    Ok(())
}

/// Print the build info line if available.
pub fn log_startup() {
    init_logging();
    info!(
        "starting rakuyomi, version: {}",
        get_build_info()
            .map(|info| info.format_display())
            .unwrap_or_else(|| "unknown".into())
    );
}
