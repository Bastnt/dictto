//! XDG base directories, read from the environment like GLib does.
use std::ffi::OsString;
use std::path::PathBuf;

/// Same folder name as the Windows app's `%LOCALAPPDATA%\com.dictto.app`.
const APP_DIR: &str = "com.dictto.app";

pub fn data_dir() -> PathBuf {
    data_dir_in(std::env::var_os("XDG_DATA_HOME"), home())
}

/// `$XDG_CONFIG_HOME`, where the autostart folder lives.
pub fn config_dir() -> PathBuf {
    xdg_base(std::env::var_os("XDG_CONFIG_HOME"), home(), ".config")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn data_dir_in(xdg_data_home: Option<OsString>, home: Option<PathBuf>) -> PathBuf {
    xdg_base(xdg_data_home, home, ".local/share").join(APP_DIR)
}

/// The spec says a relative value is invalid and must be ignored.
fn xdg_base(value: Option<OsString>, home: Option<PathBuf>, fallback: &str) -> PathBuf {
    match value.map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        _ => home.unwrap_or_else(|| PathBuf::from("/")).join(fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_respects_xdg_data_home() {
        let home = Some(PathBuf::from("/home/ada"));
        assert_eq!(
            data_dir_in(Some("/data/xdg".into()), home.clone()),
            PathBuf::from("/data/xdg/com.dictto.app")
        );
        assert_eq!(data_dir_in(None, home.clone()), PathBuf::from("/home/ada/.local/share/com.dictto.app"));
        assert_eq!(
            data_dir_in(Some("relative".into()), home),
            PathBuf::from("/home/ada/.local/share/com.dictto.app")
        );
    }

    #[test]
    fn config_dir_falls_back_to_dot_config() {
        assert_eq!(xdg_base(None, Some("/home/ada".into()), ".config"), PathBuf::from("/home/ada/.config"));
        assert_eq!(xdg_base(Some("/cfg".into()), None, ".config"), PathBuf::from("/cfg"));
    }
}
