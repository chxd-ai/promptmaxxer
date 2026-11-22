//! One-line log to `~/Library/Logs/promptmaxxer.log`. Never log keys, and never the selected text.
use std::io::Write;

pub fn log(msg: &str) {
    if cfg!(test) {
        return; // unit tests never touch the real log
    }
    let line = format!("{} {}\n", chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"), msg);
    let path = crate::paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}
