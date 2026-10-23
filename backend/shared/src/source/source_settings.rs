use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::Mutex;

use anyhow::Result;

use crate::{settings::SourceSettingValue, source_manager::SourceManager};

use super::model::SettingDefinition;

/// Callback used to persist values written by a source through `defaults.set`
/// (e.g. a refreshed OAuth token pair) without touching any lock from inside
/// the WASM call. The server registers one that forwards the change to a
/// background task, which then updates `settings.json` and the source manager.
pub type DefaultsPersister = Box<dyn Fn(String, String, SourceSettingValue) + Send + Sync>;

static DEFAULTS_PERSISTER: OnceLock<DefaultsPersister> = OnceLock::new();

/// Registers the global [`DefaultsPersister`]. Only the first call wins.
pub fn set_defaults_persister(persister: DefaultsPersister) {
    let _ = DEFAULTS_PERSISTER.set(persister);
}

#[derive(Clone)]
pub struct SourceSettings {
    source_id: String,
    defaults: HashMap<String, SourceSettingValue>,
    #[cfg(feature = "all")]
    stored: RefCell<HashMap<String, SourceSettingValue>>,
    #[cfg(not(feature = "all"))]
    pub stored: RefCell<HashMap<String, SourceSettingValue>>,
    arc_manager: Arc<Mutex<SourceManager>>,
}
impl std::fmt::Debug for SourceSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceSettings")
            .field("source_id", &self.source_id)
            .field("defaults", &self.defaults)
            .field("stored", &self.stored)
            // SourceManager はデバッグ対象外（内部状態が重すぎる）
            .field("arc_manager", &"<SourceManager>")
            .finish()
    }
}

impl SourceSettings {
    pub fn new(
        source_id: String,
        setting_definitions: &[SettingDefinition],
        stored_settings: &HashMap<String, SourceSettingValue>,
        arc_manager: &Arc<Mutex<SourceManager>>,
    ) -> Result<Self> {
        let defaults: HashMap<_, _> = setting_definitions
            .iter()
            .flat_map(default_values_for_definition)
            .collect();

        crate::settings::linked::note_source_loaded(&source_id, stored_settings);
        Ok(Self {
            source_id,
            defaults,
            stored: RefCell::new(stored_settings.clone()),
            arc_manager: arc_manager.clone(),
        })
    }

    pub fn get(&self, key: &String) -> Option<SourceSettingValue> {
        if let Some(value) = crate::settings::linked::live_value(&self.source_id, key) {
            return Some(value);
        }
        self.stored
            .borrow()
            .get(key)
            .cloned()
            .or_else(|| self.defaults.get(key).cloned())
    }

    pub fn set(&self, key: &str, value: SourceSettingValue) {
        self.stored.borrow_mut().insert(key.to_owned(), value);
    }

    /// Merges the definition defaults into the stored map (missing keys
    /// only). Used by backends whose setting definitions are only known
    /// once their runtime is up (MangaYomi: the container must be handed
    /// to the runtime first, the definitions come from the first call).
    pub fn seed_defaults(&self, setting_definitions: &[SettingDefinition]) {
        let defaults: HashMap<_, _> = setting_definitions
            .iter()
            .flat_map(default_values_for_definition)
            .collect();
        let mut stored = self.stored.borrow_mut();
        for (key, value) in defaults {
            stored.entry(key).or_insert(value);
        }
    }

    /// All effective values: stored values overlaid on the definition
    /// defaults.
    pub fn all(&self) -> HashMap<String, SourceSettingValue> {
        let mut out = self.defaults.clone();
        for (key, value) in self.stored.borrow().iter() {
            out.insert(key.clone(), value.clone());
        }
        for key in crate::settings::linked::LINKED_KEYS {
            if let Some(value) = crate::settings::linked::live_value(&self.source_id, key) {
                out.insert((*key).to_owned(), value);
            }
        }
        out
    }

    pub fn save(&self, key: &str, value: SourceSettingValue) -> Result<()> {
        // Linked sources (one account) must see e.g. a rotated token pair now.
        crate::settings::linked::publish_live(&self.source_id, key, &value);
        if let Some(persister) = DEFAULTS_PERSISTER.get() {
            // Keep the running instance consistent right away, and let the
            // persister write it to disk asynchronously. Locking the source
            // manager here could deadlock: some callers keep it locked while
            // the source is running.
            self.stored
                .borrow_mut()
                .insert(key.to_owned(), value.clone());
            persister(self.source_id.clone(), key.to_owned(), value);
            return Ok(());
        }

        let snapshot = {
            let mut store = self.stored.borrow_mut();
            store.insert(key.to_owned(), value);
            store.clone()
        };

        let mut manager = self.arc_manager.blocking_lock();
        manager.update_source_setting(self.source_id.clone(), snapshot, &self.arc_manager)?;

        Ok(())
    }
}
// only pub use in aidoku android
pub fn default_values_for_definition(
    setting_definition: &SettingDefinition,
) -> HashMap<String, SourceSettingValue> {
    match setting_definition {
        SettingDefinition::Group { items, .. } => items
            .iter()
            .flat_map(default_values_for_definition)
            .collect(),
        SettingDefinition::Select {
            key,
            default,
            values,
            ..
        } => HashMap::from([(
            key.clone(),
            SourceSettingValue::String(
                default
                    .clone()
                    .unwrap_or_else(|| values.first().cloned().unwrap_or_default()),
            ),
        )]),
        SettingDefinition::MultiSelect { key, default, .. } => {
            HashMap::from([(key.clone(), SourceSettingValue::Vec(default.clone()))])
        }
        SettingDefinition::EditableList { key, default, .. } => {
            HashMap::from([(key.clone(), SourceSettingValue::Vec(default.clone()))])
        }
        SettingDefinition::Switch { key, default, .. } => {
            HashMap::from([(key.clone(), SourceSettingValue::Bool(*default))])
        }
        // FIXME use `if let` guard when they become stable
        SettingDefinition::Text { key, default, .. } if default.is_some() => HashMap::from([(
            key.clone(),
            SourceSettingValue::String(default.clone().unwrap()),
        )]),
        _ => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        settings::{Settings, SourceSettingValue},
        source::model::SettingDefinition,
        source_manager::SourceManager,
    };
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    use super::SourceSettings;

    #[test]
    fn it_defaults_to_definition_value_if_no_stored_setting_is_present() {
        let stored_settings = HashMap::new();
        let definition = SettingDefinition::Switch {
            title: "Ok?".into(),
            key: "ok".into(),
            default: true,
        };

        let source_settings = SourceSettings::new(
            "".to_owned(),
            &[definition],
            &stored_settings,
            &Arc::new(tokio::sync::Mutex::new(SourceManager::new(
                PathBuf::new(),
                HashMap::new(),
                Settings::default(),
            ))),
        )
        .unwrap();

        assert_eq!(
            Some(SourceSettingValue::Bool(true)),
            source_settings.get(&"ok".into())
        );
    }

    #[test]
    fn it_retrieves_stored_setting_value_if_present() {
        let mut stored_settings = HashMap::new();
        stored_settings.insert("ok".into(), SourceSettingValue::Bool(false));

        let definition = SettingDefinition::Switch {
            title: "Ok?".into(),
            key: "ok".into(),
            default: true,
        };

        let source_settings = SourceSettings::new(
            "".to_owned(),
            &[definition],
            &stored_settings,
            &Arc::new(tokio::sync::Mutex::new(SourceManager::new(
                PathBuf::new(),
                HashMap::new(),
                Settings::default(),
            ))),
        )
        .unwrap();

        assert_eq!(
            Some(SourceSettingValue::Bool(false)),
            source_settings.get(&"ok".into())
        );
    }
}
