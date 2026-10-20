
use anyhow::Result;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn logs_dir() -> PathBuf {
    if cfg!(target_os = "vita") {
        PathBuf::from("ux0:/data/opennow-vita/logs")
    } else {
        PathBuf::from("opennow-vita/logs")
    }
}

pub fn frame_stats_path() -> PathBuf {
    if cfg!(target_os = "vita") {
        PathBuf::from("ux0:data/opennow-vita/frame_stats.log")
    } else {
        PathBuf::from("opennow-vita/frame_stats.log")
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
    // Like VitaForge, keep diagnostic file I/O off the rendering thread. A bounded
    // queue drops samples under load instead of turning logging into frame stalls.
    static WRITER: std::sync::OnceLock<std::sync::mpsc::SyncSender<String>> = std::sync::OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(8);
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                write_frame_stats_sync(&message);
            }
        });
        sender
    });
    let _ = writer.try_send(message.to_owned());
}

fn write_frame_stats_sync(message: &str) {
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
    let old_dir = if cfg!(target_os = "vita") {
        PathBuf::from("ux0:/data/opennow")
    } else {
        PathBuf::from("opennow")
    };
    let archive = dir.parent().expect("logs have a data directory").join("legacy-logs");
    if let Err(error) = migrate_logs(&old_dir, &archive) {
        eprintln!("Could not move previous OpenNOW logs: {error}");
    }

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

fn migrate_logs(old_dir: &std::path::Path, archive: &std::path::Path) -> std::io::Result<()> {
    if old_dir.exists() && !archive.exists() {
        fs::rename(old_dir, archive)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_logs_and_does_not_overwrite_an_archive() {
        let root = std::env::temp_dir().join(format!("opennow-logs-{}", std::process::id()));
        let old = root.join("opennow");
        let archive = root.join("opennow-vita/legacy-logs");
        fs::create_dir_all(old.join("logs")).unwrap();
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(old.join("logs/previous.log"), "previous session").unwrap();
        migrate_logs(&old, &archive).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(archive.join("logs/previous.log")).unwrap(), "previous session");
        migrate_logs(&old, &archive).unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("other.log"), "keep").unwrap();
        migrate_logs(&old, &archive).unwrap();
        assert!(old.join("other.log").exists());
        fs::remove_dir_all(root).unwrap();
    }
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
