//! The default microphone as PipeWire's `default` metadata holds it, written the way
//! `wpctl set-default` and GNOME Settings (through pipewire-pulse) do.
use crate::audio::devices::VIRTUAL_MIC_NODE;
use crate::audio::{default_name, Pipewire};
use dictto_core::speech::route::Defaults;
use std::sync::Arc;

/// The user's choice; WirePlumber derives the effective default from it.
pub const CONFIGURED_SOURCE: &str = "default.configured.audio.source";
/// What WirePlumber picked when nothing is configured.
pub const EFFECTIVE_SOURCE: &str = "default.audio.source";
/// Stands for "nothing configured" in the core journal, so a restore deletes the key and
/// WirePlumber keeps choosing automatically. Node names are never empty.
pub const UNSET: &str = "";

/// Subject-0 properties of the `default` metadata, values as raw JSON; `None` deletes.
pub trait MetadataStore: Send + Sync + 'static {
    fn get(&self, key: &'static str) -> Result<Option<String>, String>;
    fn set(&self, key: &'static str, value: Option<String>) -> Result<(), String>;
}

pub fn metadata_value(name: &str) -> String {
    serde_json::json!({ "name": name }).to_string()
}

#[derive(Clone)]
pub struct PipewireMetadata(pub Arc<Pipewire>);

impl MetadataStore for PipewireMetadata {
    fn get(&self, key: &'static str) -> Result<Option<String>, String> {
        self.0.metadata_get(key)
    }
    fn set(&self, key: &'static str, value: Option<String>) -> Result<(), String> {
        self.0.metadata_set(key, value)
    }
}

/// What the GNOME route needs beyond the core `Defaults`.
pub trait GnomeDefaults: Defaults + Clone + Send + Sync + 'static {
    /// Deletes a configured source that names Dictto's node; returns whether it did.
    fn forget_virtual(&self) -> Result<bool, String>;
}

#[derive(Clone)]
pub struct PipewireDefaults<S = PipewireMetadata> {
    store: S,
}

impl<S: MetadataStore> PipewireDefaults<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
    fn configured(&self) -> Result<Option<String>, String> {
        Ok(self.store.get(CONFIGURED_SOURCE)?.as_deref().and_then(default_name))
    }
}

impl<S: MetadataStore> Defaults for PipewireDefaults<S> {
    fn current(&self) -> Result<String, String> {
        if let Some(name) = self.configured()? {
            return Ok(name);
        }
        match self.store.get(EFFECTIVE_SOURCE)?.as_deref().and_then(default_name) {
            Some(_) => Ok(UNSET.into()),
            None => Err("No default microphone is selected.".into()),
        }
    }
    fn target(&self) -> Result<String, String> {
        Ok(VIRTUAL_MIC_NODE.into())
    }
    fn set(&self, id: &str) -> Result<(), String> {
        let value = (id != UNSET).then(|| metadata_value(id));
        self.store.set(CONFIGURED_SOURCE, value)
    }
    fn system_name(&self) -> &'static str {
        "PipeWire"
    }
    fn input_name(&self) -> &'static str {
        "default microphone"
    }
}

impl<S: MetadataStore + Clone> GnomeDefaults for PipewireDefaults<S> {
    fn forget_virtual(&self) -> Result<bool, String> {
        if self.configured()?.as_deref() != Some(VIRTUAL_MIC_NODE) {
            return Ok(false);
        }
        self.store.set(CONFIGURED_SOURCE, None)?;
        Ok(true)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// An in-memory metadata store; `log` records writes next to other fakes' calls.
    #[derive(Clone, Default)]
    pub(crate) struct Store {
        pub values: Arc<Mutex<HashMap<&'static str, String>>>,
        pub log: Arc<Mutex<Vec<&'static str>>>,
    }
    impl Store {
        pub fn with_source(configured: Option<&str>) -> Self {
            let store = Self::default();
            let mut values = store.values.lock().unwrap();
            values.insert(EFFECTIVE_SOURCE, metadata_value("alsa_input.pci"));
            if let Some(name) = configured {
                values.insert(CONFIGURED_SOURCE, metadata_value(name));
            }
            drop(values);
            store
        }
        pub fn configured(&self) -> Option<String> {
            self.values.lock().unwrap().get(CONFIGURED_SOURCE).and_then(|v| default_name(v))
        }
    }
    impl MetadataStore for Store {
        fn get(&self, key: &'static str) -> Result<Option<String>, String> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &'static str, value: Option<String>) -> Result<(), String> {
            let mut values = self.values.lock().unwrap();
            match value {
                Some(value) => {
                    self.log.lock().unwrap().push("set");
                    values.insert(key, value);
                }
                None => {
                    self.log.lock().unwrap().push("clear");
                    values.remove(key);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Store;
    use super::*;
    use dictto_core::speech::route;

    const USB: &str = "alsa_input.usb-Blue_Yeti-00.analog-stereo";

    fn journal(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dictto-gnome-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn default_metadata_value_round_trips() {
        let value = metadata_value(USB);
        assert_eq!(value, format!(r#"{{"name":"{USB}"}}"#));
        assert_eq!(default_name(&value).as_deref(), Some(USB));
        let quoted = metadata_value(r#"odd "name""#);
        assert_eq!(default_name(&quoted).as_deref(), Some(r#"odd "name""#));
    }

    #[test]
    fn current_prefers_the_configured_source() {
        let store = Store::default();
        let defaults = PipewireDefaults::new(store.clone());
        assert!(defaults.current().is_err());
        store.set(EFFECTIVE_SOURCE, Some(metadata_value("auto"))).unwrap();
        assert_eq!(defaults.current().unwrap(), UNSET);
        store.set(CONFIGURED_SOURCE, Some(metadata_value(USB))).unwrap();
        assert_eq!(defaults.current().unwrap(), USB);
    }

    #[test]
    fn manual_change_wins_over_restore() {
        let journal = journal("route");
        let store = Store::with_source(Some(USB));
        let defaults = PipewireDefaults::new(store.clone());
        assert_eq!(route::acquire(&journal, &defaults).unwrap(), VIRTUAL_MIC_NODE);
        assert_eq!(defaults.current().unwrap(), VIRTUAL_MIC_NODE);
        store.set(CONFIGURED_SOURCE, Some(metadata_value("bluez_input.headset"))).unwrap();
        route::recover(&journal, &defaults).unwrap();
        assert_eq!(defaults.current().unwrap(), "bluez_input.headset");
        assert!(!journal.exists());

        // Without a manual change the configured source is written back.
        route::acquire(&journal, &defaults).unwrap();
        route::recover(&journal, &defaults).unwrap();
        assert_eq!(store.configured().as_deref(), Some("bluez_input.headset"));
    }

    #[test]
    fn automatic_default_is_restored_as_automatic() {
        let journal = journal("unset");
        let store = Store::with_source(None);
        let defaults = PipewireDefaults::new(store.clone());
        route::acquire(&journal, &defaults).unwrap();
        assert_eq!(store.configured().as_deref(), Some(VIRTUAL_MIC_NODE));
        route::recover(&journal, &defaults).unwrap();
        assert_eq!(store.configured(), None);
        assert!(!journal.exists());
    }

    #[test]
    fn only_a_configured_virtual_mic_is_forgotten() {
        let store = Store::with_source(Some(USB));
        let defaults = PipewireDefaults::new(store.clone());
        assert!(!defaults.forget_virtual().unwrap());
        assert_eq!(store.configured().as_deref(), Some(USB));
        let stale = Store::with_source(Some(VIRTUAL_MIC_NODE));
        assert!(PipewireDefaults::new(stale.clone()).forget_virtual().unwrap());
        assert_eq!(stale.configured(), None);
    }
}
