//! Native Dictto for Fedora GNOME. Linux-only modules are gated so the workspace builds on Windows.

#[cfg(target_os = "linux")]
pub mod audio;
#[cfg(target_os = "linux")]
pub mod autostart;
#[cfg(target_os = "linux")]
pub mod daemon;
#[cfg(target_os = "linux")]
pub mod dbus;
#[cfg(target_os = "linux")]
pub mod injector;
#[cfg(target_os = "linux")]
pub mod keyboard;
#[cfg(target_os = "linux")]
pub mod notify;
#[cfg(target_os = "linux")]
pub mod paths;
#[cfg(target_os = "linux")]
pub mod session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Daemon,
    Autostart,
    Settings,
    SmokeTest,
}

impl Mode {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Mode, String> {
        let mut mode = Mode::Daemon;
        for arg in args {
            mode = match arg.as_str() {
                "--autostart" => Mode::Autostart,
                "--settings" => Mode::Settings,
                "--smoke-test" => Mode::SmokeTest,
                other => return Err(format!("unknown argument: {other}")),
            };
        }
        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;

    fn parse(args: &[&str]) -> Result<Mode, String> {
        Mode::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn modes() {
        assert_eq!(parse(&[]), Ok(Mode::Daemon));
        assert_eq!(parse(&["--autostart"]), Ok(Mode::Autostart));
        assert_eq!(parse(&["--settings"]), Ok(Mode::Settings));
        assert_eq!(parse(&["--smoke-test"]), Ok(Mode::SmokeTest));
        assert!(parse(&["--nope"]).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn keyring_default_is_persistent_not_the_mock() {
        use keyring::credential::CredentialPersistence;
        let builder = keyring::default::default_credential_builder();
        assert!(matches!(builder.persistence(), CredentialPersistence::UntilDelete));
    }
}
