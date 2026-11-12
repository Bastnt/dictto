//! Login start through the XDG autostart folder, which GNOME reads at session start.
use std::io;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "com.dictto.App.desktop";

fn file() -> PathBuf {
    crate::paths::config_dir().join("autostart").join(FILE_NAME)
}

pub fn enable(exe: &Path) -> Result<(), String> {
    write(&file(), &desktop_entry(exe)).map_err(|e| format!("Cannot write the autostart entry: {e}"))
}

pub fn disable() -> Result<(), String> {
    match std::fs::remove_file(file()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(format!("Cannot remove the autostart entry: {e}"))
        }
        _ => Ok(()),
    }
}

pub fn is_enabled() -> bool {
    file().is_file()
}

/// Written beside the target and renamed, so a session starting meanwhile never reads
/// half an entry.
fn write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("desktop.partial");
    std::fs::write(&partial, contents)?;
    std::fs::rename(&partial, path)
}

fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Dictto\n\
         Comment=Voice dictation at the cursor\n\
         Exec={} --autostart\n\
         Icon=com.dictto.App\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_arg(&exe.to_string_lossy())
    )
}

/// Quotes one Exec argument per the Desktop Entry spec: field codes escape `%`, reserved
/// characters need double quotes, and the value's own string escaping then doubles every
/// backslash produced by the quoting.
fn exec_arg(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    let reserved = |c: char| " \t\n\"'\\><~|&;$*?#()`".contains(c);
    if !arg.chars().any(reserved) {
        return arg;
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_file_contents() {
        assert_eq!(
            desktop_entry(Path::new("/home/ada/.local/bin/dictto-gnome")),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Dictto\n\
             Comment=Voice dictation at the cursor\n\
             Exec=/home/ada/.local/bin/dictto-gnome --autostart\n\
             Icon=com.dictto.App\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n"
        );
    }

    #[test]
    fn exec_paths_with_reserved_characters_are_quoted() {
        assert_eq!(exec_arg("/opt/My Apps/dictto"), "\"/opt/My Apps/dictto\"");
        assert_eq!(exec_arg("/opt/100%/d"), "/opt/100%%/d");
        // `$` becomes `\$` for the Exec parser, then `\\$` once string-escaped.
        assert_eq!(exec_arg("/a$b"), "\"/a\\\\$b\"");
    }

    #[test]
    fn write_replaces_the_entry() {
        let dir = std::env::temp_dir().join(format!("dictto-autostart-{}", std::process::id()));
        let path = dir.join("autostart").join(FILE_NAME);
        write(&path, "one").unwrap();
        write(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
