//! Sources that share one account (and therefore one OAuth token pair).
//!
//! The lib.social sites (MangaLib, HentaiLib, ...) use the same auth server,
//! and refreshing a token pair revokes the previous refresh token. Sources of
//! such a group must therefore always see the same, newest pair: whenever one
//! of them stores a token, it is mirrored into the settings of the others.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use super::{Settings, SourceSettingValue};

/// Groups of source ids that share their credentials.
pub const LINKED_SOURCE_GROUPS: &[&[&str]] = &[&["ru.mangalib.tokens", "ru.hentailib.tokens"]];

/// Stored-setting keys that are shared within a group.
pub const LINKED_KEYS: &[&str] = &["accessToken", "refreshToken", "authJson", "authClientId"];

/// Switch setting: when `true` the source keeps its own credentials (another
/// account), nothing is mirrored from or into it.
pub const SEPARATE_ACCOUNT_KEY: &str = "separateAccount";

fn is_separate(stored: Option<&HashMap<String, SourceSettingValue>>) -> bool {
    matches!(
        stored.and_then(|stored| stored.get(SEPARATE_ACCOUNT_KEY)),
        Some(SourceSettingValue::Bool(true))
    )
}

fn is_separate_in(settings: &Settings, source_id: &str) -> bool {
    is_separate(settings.source_settings.get(source_id))
}

fn linked_sources(source_id: &str) -> impl Iterator<Item = &'static str> + '_ {
    LINKED_SOURCE_GROUPS
        .iter()
        .filter(move |group| group.contains(&source_id))
        .flat_map(|group| group.iter().copied())
        .filter(move |id| *id != source_id)
}

fn is_linked_key(key: &str) -> bool {
    LINKED_KEYS.contains(&key)
}

/// Mirrors `key = value`, just stored for `source_id`, into the linked sources.
/// Returns whether anything changed.
pub fn propagate_key(
    settings: &mut Settings,
    source_id: &str,
    key: &str,
    value: &SourceSettingValue,
) -> bool {
    if !is_linked_key(key) || is_separate_in(settings, source_id) {
        return false;
    }
    let mut changed = false;
    let others: Vec<_> = linked_sources(source_id)
        .filter(|other| !is_separate_in(settings, other))
        .collect();
    for other in others {
        let stored = settings
            .source_settings
            .entry(other.to_owned())
            .or_default();
        if stored.get(key) != Some(value) {
            stored.insert(key.to_owned(), value.clone());
            changed = true;
        }
    }
    changed
}

/// Mirrors the linked keys of `source_id` that differ from `previous` (its
/// stored settings before the update) into the linked sources.
pub fn propagate_changes(
    settings: &mut Settings,
    source_id: &str,
    previous: Option<&HashMap<String, SourceSettingValue>>,
) -> bool {
    let Some(current) = settings.source_settings.get(source_id).cloned() else {
        return false;
    };
    let mut changed = false;
    for key in LINKED_KEYS {
        let Some(value) = current.get(*key) else {
            continue;
        };
        if previous.and_then(|p| p.get(*key)) == Some(value) {
            continue;
        }
        changed |= propagate_key(settings, source_id, key, value);
    }
    changed
}

fn has_credentials(stored: Option<&HashMap<String, SourceSettingValue>>) -> bool {
    stored.is_some_and(|stored| {
        ["accessToken", "refreshToken", "authJson"]
            .iter()
            .any(|key| matches!(stored.get(*key), Some(SourceSettingValue::String(s)) if !s.trim().is_empty()))
    })
}

/// Gives the members of a group without credentials the ones of a member that
/// has them (e.g. after the user configured only one of the sources).
/// Returns whether anything changed.
pub fn fill_missing(settings: &mut Settings) -> bool {
    let mut changed = false;
    for group in LINKED_SOURCE_GROUPS {
        let Some(donor) = group.iter().find(|id| {
            !is_separate_in(settings, id) && has_credentials(settings.source_settings.get(**id))
        }) else {
            continue;
        };
        let donor_settings = settings.source_settings[*donor].clone();
        let members: Vec<&str> = group
            .iter()
            .copied()
            .filter(|id| id != donor && !is_separate_in(settings, id))
            .collect();
        for id in members {
            if has_credentials(settings.source_settings.get(id)) {
                continue;
            }
            let stored = settings.source_settings.entry(id.to_owned()).or_default();
            for key in LINKED_KEYS {
                if let Some(value) = donor_settings.get(*key) {
                    stored.insert((*key).to_owned(), value.clone());
                    changed = true;
                }
            }
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Live mirroring between the running source instances
// ---------------------------------------------------------------------------
//
// `settings.json` and the loaded sources are updated asynchronously after a
// source stores a value (see the server's defaults persister). Until then the
// other members of the group would still see the previous token pair, and
// could try to refresh with an already revoked refresh token (e.g. when a
// search runs on all sources at once). Values stored by a running source are
// therefore also published here, and read by the other members first.

/// How long a published value overrides the loaded settings of the other
/// members; the persister applies it to them well before that.
const PENDING_TTL: Duration = Duration::from_secs(120);

type Pending = HashMap<String, HashMap<String, (SourceSettingValue, Instant)>>;

static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(Default::default);
static SEPARATE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

fn separate_now(source_id: &str) -> bool {
    SEPARATE
        .lock()
        .map(|set| set.contains(source_id))
        .unwrap_or(false)
}

/// Called whenever a source instance is (re)created with `stored` settings:
/// drops the published values that it already has (or that are too old).
pub fn note_source_loaded(source_id: &str, stored: &HashMap<String, SourceSettingValue>) {
    if !LINKED_SOURCE_GROUPS.iter().any(|g| g.contains(&source_id)) {
        return;
    }
    if let Ok(mut set) = SEPARATE.lock() {
        if is_separate(Some(stored)) {
            set.insert(source_id.to_owned());
        } else {
            set.remove(source_id);
        }
    }
    if let Ok(mut pending) = PENDING.lock() {
        if let Some(values) = pending.get_mut(source_id) {
            values.retain(|key, (value, at)| {
                at.elapsed() < PENDING_TTL && stored.get(key) != Some(value)
            });
        }
    }
}

/// Publishes `key = value`, just stored by the running `source_id`, to the
/// running instances of the linked sources.
pub fn publish_live(source_id: &str, key: &str, value: &SourceSettingValue) {
    if !is_linked_key(key) || separate_now(source_id) {
        return;
    }
    let others: Vec<_> = linked_sources(source_id)
        .filter(|other| !separate_now(other))
        .collect();
    if let Ok(mut pending) = PENDING.lock() {
        for other in others {
            pending
                .entry(other.to_owned())
                .or_default()
                .insert(key.to_owned(), (value.clone(), Instant::now()));
        }
    }
}

/// The value published for `source_id` by a linked source, if any.
pub fn live_value(source_id: &str, key: &str) -> Option<SourceSettingValue> {
    if !is_linked_key(key) || separate_now(source_id) {
        return None;
    }
    let pending = PENDING.lock().ok()?;
    let (value, at) = pending.get(source_id)?.get(key)?;
    (at.elapsed() < PENDING_TTL).then(|| value.clone())
}

/// Forgets the published values of the group of `source_id` (the user just
/// entered settings explicitly, they win).
pub fn clear_live(source_id: &str) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.remove(source_id);
        for other in linked_sources(source_id) {
            pending.remove(other);
        }
    }
}

/// Accepts the token JSON copied from the site for linked (token) sources:
/// `copy(JSON.stringify(JSON.parse(localStorage.auth).token))` gives
/// `{"access_token", "refresh_token", "token_type", "expires_in", ...}` and the
/// whole `localStorage.auth` is `{"token": {...}, ...}`. Such an import file is
/// rewritten to the `accessToken`/`refreshToken` settings; anything else is
/// returned unchanged.
pub fn normalize_import(
    source_id: &str,
    mut object: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    if !LINKED_SOURCE_GROUPS.iter().any(|g| g.contains(&source_id)) {
        return object;
    }
    let token = match object.get("token") {
        Some(serde_json::Value::Object(token)) => token.clone(),
        _ => object.clone(),
    };
    let access = token
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let refresh = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    if access.is_none() && refresh.is_none() {
        return object;
    }
    for key in [
        "token",
        "auth",
        "access_token",
        "refresh_token",
        "token_type",
        "expires_in",
        "timestamp",
    ] {
        object.remove(key);
    }
    if let Some(access) = access {
        object.insert("accessToken".into(), access.into());
    }
    if let Some(refresh) = refresh {
        object.insert("refreshToken".into(), refresh.into());
    }
    object
}

#[cfg(test)]
mod tests {
    use super::*;

    const ML: &str = "ru.mangalib.tokens";
    const HL: &str = "ru.hentailib.tokens";

    #[test]
    fn token_json_from_the_site_is_imported_as_tokens() {
        let raw = |json: &str| serde_json::from_str::<serde_json::Map<_, _>>(json).unwrap();
        let expected = raw(r#"{"accessToken":"a","refreshToken":"r"}"#);
        let console = raw(
            r#"{"token_type":"Bearer","expires_in":2678400,"access_token":"a","refresh_token":"r","timestamp":1}"#,
        );
        assert_eq!(normalize_import(ML, console), expected);
        let whole = raw(r#"{"token":{"access_token":"a","refresh_token":"r"},"auth":{"id":1}}"#);
        assert_eq!(normalize_import(ML, whole), expected);
        let plain = raw(r#"{"accessToken":"x","separateAccount":true}"#);
        assert_eq!(normalize_import(HL, plain.clone()), plain);
        let other = raw(r#"{"access_token":"a"}"#);
        assert_eq!(normalize_import("en.other", other.clone()), other);
    }

    fn s(value: &str) -> SourceSettingValue {
        SourceSettingValue::String(value.to_owned())
    }

    fn get<'a>(settings: &'a Settings, id: &str, key: &str) -> Option<&'a SourceSettingValue> {
        settings.source_settings.get(id).and_then(|m| m.get(key))
    }

    #[test]
    fn propagates_only_linked_keys_within_group() {
        let mut settings = Settings::default();
        assert!(propagate_key(&mut settings, ML, "refreshToken", &s("r2")));
        assert_eq!(get(&settings, HL, "refreshToken"), Some(&s("r2")));
        assert!(!propagate_key(&mut settings, ML, "refreshToken", &s("r2")));
        assert!(!propagate_key(&mut settings, ML, "url", &s("x")));
        assert!(get(&settings, HL, "url").is_none());
        assert!(!propagate_key(
            &mut settings,
            "en.other",
            "refreshToken",
            &s("r")
        ));
        assert!(!settings.source_settings.contains_key("en.other"));
    }

    #[test]
    fn propagates_changed_keys_only() {
        let mut settings = Settings::default();
        let previous: HashMap<_, _> = [("accessToken".to_owned(), s("a1"))].into();
        settings.source_settings.insert(
            HL.to_owned(),
            [
                ("accessToken".to_owned(), s("a1")),
                ("refreshToken".to_owned(), s("r1")),
            ]
            .into(),
        );
        settings
            .source_settings
            .insert(ML.to_owned(), [("accessToken".to_owned(), s("a0"))].into());
        assert!(propagate_changes(&mut settings, HL, Some(&previous)));
        // accessToken did not change in HL, so ML keeps its own value.
        assert_eq!(get(&settings, ML, "accessToken"), Some(&s("a0")));
        assert_eq!(get(&settings, ML, "refreshToken"), Some(&s("r1")));
    }

    #[test]
    fn separate_account_is_not_mirrored() {
        let mut settings = Settings::default();
        settings.source_settings.insert(
            HL.to_owned(),
            [
                (
                    SEPARATE_ACCOUNT_KEY.to_owned(),
                    SourceSettingValue::Bool(true),
                ),
                ("refreshToken".to_owned(), s("own")),
            ]
            .into(),
        );
        assert!(!propagate_key(&mut settings, ML, "refreshToken", &s("r2")));
        assert!(!propagate_key(
            &mut settings,
            HL,
            "refreshToken",
            &s("own2")
        ));
        assert_eq!(get(&settings, HL, "refreshToken"), Some(&s("own")));
        assert!(get(&settings, ML, "refreshToken").is_none());
        settings
            .source_settings
            .insert(ML.to_owned(), [("accessToken".to_owned(), s("a"))].into());
        assert!(!fill_missing(&mut settings));
        assert!(get(&settings, HL, "accessToken").is_none());
    }

    #[test]
    fn live_values_reach_running_linked_sources() {
        const A: &str = "ru.mangalib.tokens";
        const B: &str = "ru.hentailib.tokens";
        note_source_loaded(A, &HashMap::new());
        note_source_loaded(B, &[("refreshToken".to_owned(), s("old"))].into());
        publish_live(A, "refreshToken", &s("new"));
        publish_live(A, "url", &s("x"));
        assert_eq!(live_value(B, "refreshToken"), Some(s("new")));
        assert_eq!(live_value(B, "url"), None);
        assert_eq!(live_value(A, "refreshToken"), None);
        // Reloaded with the value applied: the override is gone.
        note_source_loaded(B, &[("refreshToken".to_owned(), s("new"))].into());
        assert_eq!(live_value(B, "refreshToken"), None);
        // A separate account neither publishes nor receives.
        note_source_loaded(
            B,
            &[(
                SEPARATE_ACCOUNT_KEY.to_owned(),
                SourceSettingValue::Bool(true),
            )]
            .into(),
        );
        publish_live(A, "refreshToken", &s("newer"));
        publish_live(B, "refreshToken", &s("own"));
        assert_eq!(live_value(B, "refreshToken"), None);
        assert_eq!(live_value(A, "refreshToken"), None);
        note_source_loaded(B, &HashMap::new());
        clear_live(A);
    }

    #[test]
    fn fills_missing_credentials() {
        let mut settings = Settings::default();
        settings.source_settings.insert(
            ML.to_owned(),
            [
                ("accessToken".to_owned(), s("a")),
                ("refreshToken".to_owned(), s("r")),
                ("url".to_owned(), s("https://mangalib.me")),
            ]
            .into(),
        );
        settings
            .source_settings
            .insert(HL.to_owned(), [("accessToken".to_owned(), s(" "))].into());
        assert!(fill_missing(&mut settings));
        assert_eq!(get(&settings, HL, "accessToken"), Some(&s("a")));
        assert_eq!(get(&settings, HL, "refreshToken"), Some(&s("r")));
        assert!(get(&settings, HL, "url").is_none());
        assert!(!fill_missing(&mut settings));
    }
}
