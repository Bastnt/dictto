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

/// Subject-0 properties of the `default` metadata, values as raw JSON.
pub trait MetadataStore: Send + Sync + 'static {
    fn get(&self, key: &'static str) -> Result<Option<String>, String>;
    fn set(&self, key: &'static str, value: String) -> Result<(), String>;
}

pub fn metadata_value(name: &str) -> String {
    serde_json::json!({ "name": name }).to_string()
}

pub struct PipewireMetadata(pub Arc<Pipewire>);

impl MetadataStore for PipewireMetadata {
    fn get(&self, key: &'static str) -> Result<Option<String>, String> {
        self.0.metadata_get(key)
    }
    fn set(&self, key: &'static str, value: String) -> Result<(), String> {
        self.0.metadata_set(key, value)
    }
}

pub struct PipewireDefaults<S = PipewireMetadata> {
    store: S,
}

impl<S: MetadataStore> PipewireDefaults<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

impl<S: MetadataStore> Defaults for PipewireDefaults<S> {
    /// Without a configured source the effective one is what the user hears from; it is
    /// what a restore writes back.
    fn current(&self) -> Result<String, String> {
        for key in [CONFIGURED_SOURCE, EFFECTIVE_SOURCE] {
            if let Some(name) = self.store.get(key)?.as_deref().and_then(default_name) {
                return Ok(name);
            }
        }
        Err("No default microphone is selected.".into())
    }
    fn target(&self) -> Result<String, String> {
        Ok(VIRTUAL_MIC_NODE.into())
    }
    fn set(&self, id: &str) -> Result<(), String> {
        self.store.set(CONFIGURED_SOURCE, metadata_value(id))
    }
    fn system_name(&self) -> &'static str {
        "PipeWire"
    }
    fn input_name(&self) -> &'static str {
        "default microphone"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictto_core::speech::route;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Store(Arc<Mutex<HashMap<&'static str, String>>>);
    impl MetadataStore for Store {
        fn get(&self, key: &'static str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &'static str, value: String) -> Result<(), String> {
            self.0.lock().unwrap().insert(key, value);
            Ok(())
        }
    }

    const USB: &str = "alsa_input.usb-Blue_Yeti-00.analog-stereo";

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
        store.set(EFFECTIVE_SOURCE, metadata_value("auto")).unwrap();
        assert_eq!(defaults.current().unwrap(), "auto");
        store.set(CONFIGURED_SOURCE, metadata_value(USB)).unwrap();
        assert_eq!(defaults.current().unwrap(), USB);
    }

    #[test]
    fn manual_change_wins_over_restore() {
        let journal = std::env::temp_dir().join(format!("dictto-gnome-route-{}.json", std::process::id()));
        let store = Store::default();
        store.set(CONFIGURED_SOURCE, metadata_value(USB)).unwrap();
        let defaults = PipewireDefaults::new(store.clone());
        assert_eq!(route::acquire(&journal, &defaults).unwrap(), VIRTUAL_MIC_NODE);
        assert_eq!(defaults.current().unwrap(), VIRTUAL_MIC_NODE);
        store.set(CONFIGURED_SOURCE, metadata_value("bluez_input.headset")).unwrap();
        route::recover(&journal, &defaults).unwrap();
        assert_eq!(defaults.current().unwrap(), "bluez_input.headset");
        assert!(!journal.exists());

        // Without a manual change the configured source is written back.
        route::acquire(&journal, &defaults).unwrap();
        route::recover(&journal, &defaults).unwrap();
        assert_eq!(
            store.get(CONFIGURED_SOURCE).unwrap().unwrap(),
            metadata_value("bluez_input.headset")
        );
    }
}
