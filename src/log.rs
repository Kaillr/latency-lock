//! Tiny append-only log file in %LOCALAPPDATA%\LatencyLock.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::System::SystemInformation::GetLocalTime;

static FILE: Mutex<Option<File>> = Mutex::new(None);
const MAX_BYTES: u64 = 512 * 1024;

pub fn path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("LatencyLock").join("latency-lock.log")
}

pub fn init() {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Keep the log small: roll over to a single .old.log once it grows past MAX_BYTES.
    if fs::metadata(&p)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&p, p.with_extension("old.log"));
    }
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&p) {
        *FILE.lock().unwrap() = Some(f);
    }
}

pub fn info(msg: impl AsRef<str>) {
    let t = unsafe { GetLocalTime() };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  {}\r\n",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        msg.as_ref()
    );
    if let Ok(mut guard) = FILE.lock()
        && let Some(f) = guard.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
    }
}
