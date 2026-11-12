use dictto_gnome::Mode;

fn main() {
    let mode = match Mode::parse(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("dictto-gnome: {message}");
            std::process::exit(2);
        }
    };
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mode;
        eprintln!("dictto-gnome runs on Linux only");
        std::process::exit(1);
    }
    #[cfg(target_os = "linux")]
    {
        init_logging();
        let result = match mode {
            Mode::Settings => open_settings_window(),
            Mode::Daemon | Mode::Autostart | Mode::SmokeTest => dictto_gnome::daemon::run(mode),
        };
        if let Err(error) = result {
            log::error!("{error}");
            std::process::exit(1);
        }
    }
}

/// Replaces this process with the GTK settings binary.
#[cfg(target_os = "linux")]
fn open_settings_window() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let error = dictto_gnome::settings_command()?.exec();
    Err(format!("cannot start the settings window: {error}"))
}

/// Plain lines on stderr. Under systemd (JOURNAL_STREAM set) each line carries the
/// `<N>` syslog prefix so journald records the right priority.
#[cfg(target_os = "linux")]
fn init_logging() {
    use std::io::Write;
    let journal = std::env::var_os("JOURNAL_STREAM").is_some();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .format(move |buf, record| {
            let level = record.level();
            if journal {
                let priority = match level {
                    log::Level::Error => 3,
                    log::Level::Warn => 4,
                    log::Level::Info => 6,
                    log::Level::Debug | log::Level::Trace => 7,
                };
                writeln!(buf, "<{priority}>{}: {}", record.target(), record.args())
            } else {
                writeln!(buf, "{level:<5} {}: {}", record.target(), record.args())
            }
        })
        .init();
}
