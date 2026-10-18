use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

struct FileLogger(Mutex<File>);
static LOGGER: OnceLock<FileLogger> = OnceLock::new();

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) { return; }
        if let Ok(mut file) = self.0.lock() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
            let _ = writeln!(file, "[{timestamp}][{}][{}] {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.0.lock() { let _ = file.flush(); }
    }
}

pub fn initialize() -> Result<(), Box<dyn std::error::Error>> {
    let folder = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA unavailable")?)
        .join("com.dictto.app").join("logs");
    std::fs::create_dir_all(&folder)?;
    let file = OpenOptions::new().create(true).append(true).open(folder.join("Dictto-Native.log"))?;
    let logger = LOGGER.get_or_init(|| FileLogger(Mutex::new(file)));
    log::set_logger(logger).map_err(|_| "Logger already initialized")?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
