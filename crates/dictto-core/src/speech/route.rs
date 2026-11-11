//! Crash-safe switching of the default microphone to a virtual one.
//! A journal written before each switch lets a later start restore the user's input.
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::Path;

/// The OS default input that other applications capture, and the virtual microphone
/// it is switched to.
pub trait Defaults {
    fn current(&self) -> Result<String, String>;
    fn target(&self) -> Result<String, String>;
    fn set(&self, id: &str) -> Result<(), String>;
    fn owns_route(&self, id: &str) -> Result<bool, String> {
        Ok(self.target()? == id)
    }
    /// Subject of user-facing messages, e.g. "The system has not confirmed ...".
    fn system_name(&self) -> &'static str {
        "The system"
    }
    /// What users call the switched input in this OS's settings.
    fn input_name(&self) -> &'static str {
        "default microphone"
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    original: String,
    temporary: String,
}

fn observed_after_set(defaults: &impl Defaults, previous: &str) -> Result<String, String> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let current = defaults.current()?;
        if current != previous {
            return Ok(current);
        }
        if std::time::Instant::now() >= until {
            return Err(format!(
                "{} has not confirmed the {} change.",
                defaults.system_name(),
                defaults.input_name()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub fn recover(path: &Path, defaults: &impl Defaults) -> Result<(), String> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let pending = path.with_extension("pending");
            if pending.exists() {
                fs::remove_file(pending).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        Err(e) => return Err(format!("Cannot read microphone recovery file: {e}")),
    };
    let saved: Saved = serde_json::from_slice(&data)
        .map_err(|e| format!("Invalid microphone recovery file: {e}"))?;
    if defaults.current()? == saved.temporary {
        if !defaults.owns_route(&saved.temporary)? {
            return Err("The journaled device is not a supported virtual microphone.".into());
        }
        defaults.set(&saved.original)?;
        observed_after_set(defaults, &saved.temporary)?;
    }
    fs::remove_file(path).map_err(|e| format!("Cannot clear microphone recovery file: {e}"))
}

pub fn acquire(path: &Path, defaults: &impl Defaults) -> Result<String, String> {
    recover(path, defaults)?;
    let target = defaults.target()?;
    let original = defaults.current()?;
    if original == target {
        return Ok(target);
    }
    let saved = Saved {
        original,
        temporary: target.clone(),
    };
    let pending = path.with_extension("pending");
    {
        let file = File::create(&pending).map_err(|e| e.to_string())?;
        serde_json::to_writer(&file, &saved).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&pending, path).map_err(|e| e.to_string())?;
    let current = defaults.current();
    if current.as_ref().ok() != Some(&saved.original) {
        // No policy write was issued; the endpoint can disappear or be selected manually here.
        fs::remove_file(path).map_err(|e| e.to_string())?;
        return Err(current.err().unwrap_or_else(|| {
            format!("The {} changed before routing began.", defaults.input_name())
        }));
    }
    if let Err(error) = defaults.set(&target) {
        if let Err(restore) = recover(path, defaults) {
            return Err(format!("{error} Recovery pending: {restore}"));
        }
        return Err(error);
    }
    let observed = observed_after_set(defaults, &saved.original)
        .map_err(|e| format!("{e} The recovery file was retained."))?;
    if observed != target {
        fs::remove_file(path).map_err(|e| e.to_string())?;
        return Err(format!("The user selected a different {}.", defaults.input_name()));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake(RefCell<String>);
    impl Defaults for Fake {
        fn current(&self) -> Result<String, String> {
            Ok(self.0.borrow().clone())
        }
        fn target(&self) -> Result<String, String> {
            Ok("virtual".into())
        }
        fn set(&self, id: &str) -> Result<(), String> {
            *self.0.borrow_mut() = id.into();
            Ok(())
        }
    }
    #[test]
    fn missing_prior_input_never_attempts_a_switch() {
        struct Missing;
        impl Defaults for Missing {
            fn current(&self) -> Result<String, String> {
                Err("No default capture endpoint".into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No restorable microphone")
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-missing-mic-{}.json", std::process::id()));
        assert!(acquire(&path, &Missing).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn user_selecting_virtual_before_first_policy_write_is_preserved() {
        struct Changed(std::cell::Cell<u8>);
        impl Defaults for Changed {
            fn current(&self) -> Result<String, String> {
                let n = self.0.get();
                self.0.set(n + 1);
                Ok(if n == 0 { "usb" } else { "virtual" }.into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No switch was authorized after manual selection")
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-user-mic-{}.json", std::process::id()));
        assert!(acquire(&path, &Changed(0.into())).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn input_disappearing_before_switch_leaves_no_journal() {
        struct Vanished(std::cell::Cell<bool>);
        impl Defaults for Vanished {
            fn current(&self) -> Result<String, String> {
                if self.0.replace(true) {
                    Err("Unplugged".into())
                } else {
                    Ok("usb".into())
                }
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No policy write")
            }
        }
        let path = std::env::temp_dir().join(format!(
            "dictto-unplug-before-switch-{}.json",
            std::process::id()
        ));
        assert_eq!(
            acquire(&path, &Vanished(false.into())).unwrap_err(),
            "Unplugged"
        );
        assert!(!path.exists());
    }
    #[test]
    fn unplugged_original_keeps_recovery_for_later() {
        struct Unplugged;
        impl Defaults for Unplugged {
            fn current(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                Err("Disconnected".into())
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-recover-mic-{}.json", std::process::id()));
        let policy = Fake(RefCell::new("usb".to_string()));
        acquire(&path, &policy).unwrap();
        assert!(recover(&path, &Unplugged).is_err());
        assert!(path.exists());
        recover(&path, &policy).unwrap();
        assert!(!path.exists());
        assert_eq!(policy.current().unwrap(), "usb");
    }
    #[test]
    fn restores_only_own_switch_including_after_restart() {
        let path =
            std::env::temp_dir().join(format!("dictto-route-test-{}.json", std::process::id()));
        let policy = Fake("usb".to_string().into());
        acquire(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "virtual");
        recover(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "usb");
        assert!(!path.exists());
        acquire(&path, &policy).unwrap();
        policy.set("user-chosen").unwrap();
        recover(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "user-chosen");
        policy.set("virtual").unwrap();
        acquire(&path, &policy).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn recover_restores_journaled_endpoint() {
        let path =
            std::env::temp_dir().join(format!("dictto-journal-test-{}.json", std::process::id()));
        fs::write(&path, br#"{"original":"usb","temporary":"virtual"}"#).unwrap();
        let policy = Fake("virtual".to_string().into());
        recover(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "usb");
        assert!(!path.exists());
    }
}
