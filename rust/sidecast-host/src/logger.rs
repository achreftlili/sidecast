//! A small logger for the `log` facade: `HH:MM:SS.mmm LEVEL message`, to
//! stdout (command line) or to a file (the Windows tray app has no console).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

/// A log file is moved aside (`.old.log`) at start once it is this large.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

struct HostLogger {
    verbose: bool,
    /// `None`: stdout.
    file: Option<Mutex<File>>,
}

impl Log for HostLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let ours = metadata.target().starts_with("sidecast");
        // Dependencies (TLS, DNS-SD) only report problems unless --verbose.
        let max = match (ours, self.verbose) {
            (_, true) => Level::Debug,
            (true, false) => Level::Info,
            (false, false) => Level::Warn,
        };
        metadata.level() <= max
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let secs = (millis / 1000) % 86_400;
        let level = match record.level() {
            Level::Error => "ERROR ",
            Level::Warn => "WARN  ",
            Level::Info => "",
            Level::Debug | Level::Trace => "debug ",
        };
        let line = format!(
            "{:02}:{:02}:{:02}.{:03} {level}{}\n",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60,
            millis % 1000,
            record.args()
        );
        match &self.file {
            Some(file) => {
                let mut file = file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = file.write_all(line.as_bytes());
            }
            None => {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
            }
        }
    }

    fn flush(&self) {
        match &self.file {
            Some(file) => {
                let _ = file.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).flush();
            }
            None => {
                let _ = std::io::stdout().flush();
            }
        }
    }
}

fn install(logger: HostLogger) {
    let verbose = logger.verbose;
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(if verbose { LevelFilter::Debug } else { LevelFilter::Info });
    }
}

/// Logs to stdout.
pub fn init(verbose: bool) {
    install(HostLogger { verbose, file: None });
}

/// Logs to `path` (appending; a large file is moved aside first).
pub fn init_file(verbose: bool, path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        let _ = fs::rename(path, path.with_extension("old.log"));
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    install(HostLogger { verbose, file: Some(Mutex::new(file)) });
    Ok(())
}
