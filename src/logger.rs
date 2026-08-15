
use anyhow::Result;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn logs_dir() -> PathBuf {
    if cfg!(target_os = "vita") {
        PathBuf::from("ux0:/data/opennow/logs")
    } else {
        PathBuf::from("opennow/logs")
    }
}

pub fn frame_stats_path() -> PathBuf {
    if cfg!(target_os = "vita") {
        PathBuf::from("ux0:data/opennow/frame_stats.log")
    } else {
        PathBuf::from("opennow/frame_stats.log")
    }
}

pub fn reset_frame_stats_log() {
    let path = frame_stats_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, "");
}

pub fn write_frame_stats(message: &str) {
    write_log("FRAME", message);
    let path = frame_stats_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{message}");
    }
}

pub fn init() -> Result<()> {
    let dir = logs_dir();
    fs::create_dir_all(&dir)?;

    let latest_path = dir.join("opennow_latest.log");
    let previous_path = dir.join("opennow_previous.log");

    if latest_path.exists() {
        let _ = fs::rename(&latest_path, &previous_path);
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&latest_path)?;

    if let Ok(mut guard) = LOG_FILE.lock() {
        *guard = Some(file);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("[FATAL PANIC] {info}\n");
        eprintln!("{msg}");
        write_log("FATAL", &msg);
        default_hook(info);
    }));

    write_log("INFO", "OpenNOW-vita logger initialized");
    Ok(())
}

pub fn write_log(level: &str, message: &str) {
    let timestamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    };

    let line = format!("[{timestamp}] [{level}] {message}\n");
    eprint!("{line}");

    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(ref mut file) = *guard {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logger::write_log("INFO", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::logger::write_log("WARN", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logger::write_log("ERROR", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_stream {
    ($($arg:tt)*) => {
        $crate::logger::write_log("STREAM", &format!($($arg)*))
    };
}
