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
#[cfg(target_os = "linux")]
pub mod speech;

/// Linux appends " (deleted)" to /proc/self/exe once the running binary was replaced by an
/// install; the path itself still names the freshly installed file.
pub fn strip_deleted_suffix(path: &std::path::Path) -> std::path::PathBuf {
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(clean) => std::path::PathBuf::from(clean),
        None => path.to_path_buf(),
    }
}

#[cfg(target_os = "linux")]
pub fn current_exe() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe().map(|p| strip_deleted_suffix(&p))
}

/// GTK and libadwaita live in this sibling binary: linked here, they would be mapped
/// into the always-running daemon.
#[cfg(target_os = "linux")]
pub fn settings_command() -> Result<std::process::Command, String> {
    let exe = current_exe().map_err(|e| format!("cannot locate the dictto-gnome binary: {e}"))?;
    Ok(std::process::Command::new(exe.with_file_name("dictto-gnome-settings")))
}

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

#[cfg(test)]
mod exe_path_tests {
    use super::strip_deleted_suffix;
    use std::path::Path;

    #[test]
    fn deleted_suffix_is_stripped() {
        assert_eq!(
            strip_deleted_suffix(Path::new("/home/u/.local/bin/dictto-gnome (deleted)")),
            Path::new("/home/u/.local/bin/dictto-gnome")
        );
        assert_eq!(strip_deleted_suffix(Path::new("/usr/bin/dictto-gnome")), Path::new("/usr/bin/dictto-gnome"));
    }
}
