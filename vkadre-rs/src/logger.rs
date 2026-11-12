//! File logger: %APPDATA%\VKadre\speakertrack.log, rotated at 1 MB (two old files kept).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::app_data_dir;

const MAX_BYTES: u64 = 1_000_000;

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

fn rotate(path: &PathBuf) {
    if std::fs::metadata(path).map(|m| m.len() < MAX_BYTES).unwrap_or(true) {
        return;
    }
    let old = |n: u32| PathBuf::from(format!("{}.{n}", path.display()));
    let _ = std::fs::remove_file(old(2));
    let _ = std::fs::rename(old(1), old(2));
    let _ = std::fs::rename(path, old(1));
}

fn open(path: &PathBuf) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// UTC time of day; enough to line log lines up with what happened on air.
fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let days = (secs / 86400.0).floor() as i64;
    let rem = secs - days as f64 * 86400.0;
    let (h, m, s) = ((rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, rem % 60.0);
    format!("{h:02}:{m:02}:{s:06.3}Z")
}

impl log::Log for FileLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::Level::Info || meta.target().starts_with("vkadre")
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // keep third-party chatter out
        let t = record.target();
        if !(t.starts_with("VKadre") || t.starts_with("vkadre") || record.level() <= log::Level::Warn) {
            return;
        }
        let line = format!("{} {} {}: {}\n", timestamp(), record.level(), t, record.args());
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        let mut f = self.file.lock().unwrap();
        if let Some(file) = f.as_mut() {
            let _ = file.write_all(line.as_bytes());
            if file.metadata().map(|m| m.len() >= MAX_BYTES).unwrap_or(false) {
                *f = None;
                rotate(&self.path);
                *f = open(&self.path);
            }
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.flush();
        }
    }
}

pub fn init() {
    let path = app_data_dir().join("speakertrack.log");
    rotate(&path);
    let logger = FileLogger { file: Mutex::new(open(&path)), path };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
