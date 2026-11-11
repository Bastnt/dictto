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
        // Daemon, settings window and smoke test arrive with the later tasks.
        eprintln!("dictto-gnome: mode {mode:?} not implemented yet");
    }
}
