//! Self-update from GitHub Releases, using only what ships with Windows (WinHTTP, BCrypt).
//!
//! A worker thread asks the GitHub API for the latest release. If it is newer, it downloads
//! `latency-lock.exe` and the `.sha256` published next to it, checks the hash, and puts the new
//! exe at the running exe's path. Windows won't overwrite a running exe but will rename it, so
//! the running one moves aside to `latency-lock.exe.old` and carries on from there. Nothing
//! restarts on its own: the new version runs the next time LatencyLock starts, and that start
//! deletes the `.old`.

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, PCWSTR, w};

use crate::{VERSION, log};

/// Posted to the main window when a check finishes; LPARAM owns a `Box<Report>` (see `take`).
pub const WM_UPDATE: u32 = WM_APP + 2;

const ASSET: &str = "latency-lock.exe";
const MAX_DOWNLOAD: usize = 16 * 1024 * 1024;

pub enum Outcome {
    /// Nothing newer than what is running (the latest release's version).
    UpToDate(String),
    /// This newer version is on disk and runs the next time LatencyLock starts.
    Ready(String),
    Failed(String),
}

pub struct Report {
    /// Started from the menu, so the user expects an answer even if nothing changed.
    pub manual: bool,
    pub outcome: Outcome,
}

static BUSY: AtomicBool = AtomicBool::new(false);
/// The version downloaded during this session, waiting for the next start.
static READY: Mutex<Option<String>> = Mutex::new(None);

/// Check for a newer release on a worker thread, download it if there is one, and post
/// `WM_UPDATE` to `hwnd` when done. Returns false if a check is already running.
pub fn start(hwnd: HWND, manual: bool) -> bool {
    if BUSY.swap(true, Ordering::AcqRel) {
        return false;
    }
    let target = hwnd.0 as usize; // HWND is not Send; the handle value is.
    std::thread::spawn(move || {
        let outcome = check().unwrap_or_else(Outcome::Failed);
        let report = Box::into_raw(Box::new(Report { manual, outcome }));
        let hwnd = HWND(target as *mut c_void);
        if unsafe { PostMessageW(hwnd, WM_UPDATE, WPARAM(0), LPARAM(report as isize)) }.is_err() {
            // The window is gone; nobody will take the report.
            drop(unsafe { Box::from_raw(report) });
            BUSY.store(false, Ordering::Release);
        }
    });
    true
}

/// Claim the report carried by a `WM_UPDATE` message.
///
/// # Safety
/// `lp` must be the LPARAM of a `WM_UPDATE` posted by `start`, and taken only once.
pub unsafe fn take(lp: LPARAM) -> Report {
    BUSY.store(false, Ordering::Release);
    *unsafe { Box::from_raw(lp.0 as *mut Report) }
}

/// Start the exe now on disk (the downloaded version) with `--updated`, which takes over once
/// this process exits.
pub fn launch_new() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .arg("--updated")
        .args(crate::install::mode_args())
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Delete what a previous update left next to the exe.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(sibling(&exe, "old"));
        let _ = fs::remove_file(sibling(&exe, "new"));
    }
}

fn check() -> Result<Outcome, String> {
    let repo = env!("CARGO_PKG_REPOSITORY")
        .strip_prefix("https://github.com/")
        .ok_or("the repository in Cargo.toml is not on GitHub")?;
    let release = get("api.github.com", &format!("/repos/{repo}/releases/latest"))?;
    let tag = json_string(&String::from_utf8_lossy(&release), "tag_name")
        .ok_or("the latest release has no tag")?;
    let latest = tag.trim_start_matches('v').to_owned();
    let ready = READY.lock().unwrap().clone();
    if !is_newer(&latest, ready.as_deref().unwrap_or(VERSION)) {
        return Ok(match ready {
            Some(version) => Outcome::Ready(version),
            None => Outcome::UpToDate(latest),
        });
    }
    let path = format!("/{repo}/releases/download/{tag}/{ASSET}");
    let exe = get("github.com", &path)?;
    let sums = get("github.com", &format!("{path}.sha256"))?;
    let expected = String::from_utf8_lossy(&sums)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if sha256_hex(&exe)? != expected {
        return Err(format!(
            "{ASSET} from {tag} does not match its published SHA-256"
        ));
    }
    swap_in(&exe, ready.is_some())?;
    log::info(format!(
        "Downloaded LatencyLock {latest}; it runs the next time LatencyLock starts"
    ));
    *READY.lock().unwrap() = Some(latest.clone());
    Ok(Outcome::Ready(latest))
}

/// Put `bytes` at the exe's path. The first time, the running exe is in the way and moves
/// aside to `.old`; after that the path holds an earlier download, which is simply replaced.
fn swap_in(bytes: &[u8], running_moved: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (new, old) = (sibling(&exe, "new"), sibling(&exe, "old"));
    fs::write(&new, bytes).map_err(|e| format!("cannot write {}: {e}", new.display()))?;
    if !running_moved {
        let _ = fs::remove_file(&old);
        if let Err(e) = fs::rename(&exe, &old) {
            let _ = fs::remove_file(&new);
            return Err(format!("cannot move {} aside: {e}", exe.display()));
        }
    }
    if let Err(e) = fs::rename(&new, &exe) {
        if !running_moved {
            let _ = fs::rename(&old, &exe);
        }
        let _ = fs::remove_file(&new);
        return Err(format!("cannot replace {}: {e}", exe.display()));
    }
    Ok(())
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    name.into()
}

/// WinHTTP handle, closed on drop.
struct Internet(*mut c_void);

impl Drop for Internet {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// HTTPS GET; follows redirects (release downloads redirect to GitHub's CDN).
fn get(host: &str, path: &str) -> Result<Vec<u8>, String> {
    let failed = |what: &str, e: windows::core::Error| format!("{what} {host}: {e}");
    unsafe {
        let agent = HSTRING::from(format!("LatencyLock/{VERSION}"));
        let session = Internet(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(failed(
                "cannot open a session for",
                windows::core::Error::from_win32(),
            ));
        }
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 30_000);
        let connection = Internet(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(failed(
                "cannot connect to",
                windows::core::Error::from_win32(),
            ));
        }
        let request = Internet(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(failed(
                "cannot create a request to",
                windows::core::Error::from_win32(),
            ));
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)
            .map_err(|e| failed("cannot reach", e))?;
        WinHttpReceiveResponse(request.0, null_mut()).map_err(|e| failed("no response from", e))?;

        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            null_mut(),
        )
        .map_err(|e| failed("bad response from", e))?;
        if status != 200 {
            return Err(format!("{host}{path} returned HTTP {status}"));
        }

        let mut body = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut read,
            )
            .map_err(|e| failed("download interrupted from", e))?;
            if read == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > MAX_DOWNLOAD {
                return Err(format!("{host}{path} is unexpectedly large"));
            }
        }
    }
}

fn sha256_hex(data: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut digest) }
        .ok()
        .map_err(|e| format!("SHA-256 failed: {e}"))?;
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// The string value of the first `"key": "..."` in a JSON document. Enough for the plain
/// values read here (tags), which never contain escapes.
fn json_string(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

/// `major.minor.patch`, ignoring a leading `v` and any `-pre`/`+build` suffix.
fn parse_version(v: &str) -> Option<[u64; 3]> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(a), Some(b)) if a > b
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("10.0.1-beta.2"), Some([10, 0, 1]));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("garbage", "1.0.0"));
    }

    #[test]
    fn tag_from_release_json() {
        let json = r#"{"url":"x","tag_name": "v1.4.0","body":"see \"tag_name\": \"v9\""}"#;
        assert_eq!(json_string(json, "tag_name").as_deref(), Some("v1.4.0"));
        assert_eq!(json_string(json, "missing"), None);
    }

    #[test]
    fn sibling_paths() {
        let exe = Path::new(r"C:\Tools\latency-lock.exe");
        assert_eq!(
            sibling(exe, "old"),
            PathBuf::from(r"C:\Tools\latency-lock.exe.old")
        );
    }
}
