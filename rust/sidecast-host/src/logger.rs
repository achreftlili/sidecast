//! A small stdout logger for the `log` facade: `HH:MM:SS.mmm LEVEL message`.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

struct StdoutLogger {
    verbose: bool,
}

impl Log for StdoutLogger {
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
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    fn flush(&self) {
        let _ = std::io::stdout().flush();
    }
}

pub fn init(verbose: bool) {
    if log::set_logger(Box::leak(Box::new(StdoutLogger { verbose }))).is_ok() {
        log::set_max_level(if verbose { LevelFilter::Debug } else { LevelFilter::Info });
    }
}
