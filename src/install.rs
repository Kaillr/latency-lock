//! Per-user install: the downloaded exe is its own installer.
//!
//! Started from anywhere else, latency-lock.exe copies itself to
//! %LOCALAPPDATA%\Programs\LatencyLock, adds a Start menu shortcut and an Apps & features
//! entry, turns on Start with Windows, and hands over to the installed copy. Starting a download
//! again opens the installed copy, after upgrading it if the download is newer. Updates
//! (update.rs) replace the installed exe in place. `--uninstall`, which Apps & features runs,
//! removes all of it. `--portable` (and debug builds) skip this and run the exe where it is.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, FALSE, HWND, LPARAM, TRUE, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::System::Threading::{
    OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_YESNO, PostMessageW, WM_CLOSE,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

use crate::{VERSION, log, message, settings, update};

/// Held by the running instance for its whole life (one per user session).
pub const MUTEX: PCWSTR = w!("Local\\LatencyLock.SingleInstance");
/// The running instance's hidden window, which other copies ask to close.
pub const WINDOW_CLASS: PCWSTR = w!("LatencyLockWindow");
pub const ALREADY_RUNNING: &str =
    "LatencyLock is already running. Look for the headphones icon in the notification area.";

const UNINSTALL_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\LatencyLock");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static PORTABLE: AtomicBool = AtomicBool::new(false);

pub fn set_portable() {
    PORTABLE.store(true, Ordering::Relaxed);
}

pub fn portable() -> bool {
    PORTABLE.load(Ordering::Relaxed)
}

/// Arguments that keep a restarted copy (or the Start with Windows entry) in the same mode.
pub fn mode_args() -> &'static [&'static str] {
    if portable() { &["--portable"] } else { &[] }
}

fn known_folder(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn install_dir() -> Option<PathBuf> {
    known_folder("LOCALAPPDATA").map(|d| d.join("Programs").join("LatencyLock"))
}

fn shortcut() -> Option<PathBuf> {
    known_folder("APPDATA")
        .map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\LatencyLock.lnk"))
}

/// Is this process the installed copy?
pub fn is_installed_copy() -> bool {
    let (Ok(me), Some(dir)) = (std::env::current_exe(), install_dir()) else {
        return false;
    };
    matches!(
        (fs::canonicalize(me), fs::canonicalize(dir.join("latency-lock.exe"))),
        (Ok(a), Ok(b)) if a == b
    )
}

/// Started from a download (or any copy outside the install folder): install or upgrade, then
/// start the installed copy. `updated` passes on a restart after an in-place update, so the
/// installed copy waits for the old process to exit.
pub fn run_installer(updated: bool) {
    // A log means LatencyLock has run on this account before (older versions ran in place).
    let used_before = log::path().exists();
    log::init();
    update::cleanup();
    if let Err(e) = install(used_before, updated) {
        log::info(format!("Install failed: {e}"));
        message(
            HWND::default(),
            &format!(
                "LatencyLock could not install itself:\n{e}\n\n\
                 To run it without installing, start it with --portable."
            ),
            MB_ICONWARNING,
        );
    }
}

fn install(used_before: bool, updated: bool) -> Result<(), String> {
    let dir = install_dir().ok_or("%LOCALAPPDATA% is not set")?;
    let exe = dir.join("latency-lock.exe");
    let fresh = !exe.exists();
    let installed = settings::get_string(UNINSTALL_KEY, w!("DisplayVersion"));
    let newer = fresh
        || installed
            .as_deref()
            .is_none_or(|v| update::is_newer(VERSION, v));

    if newer {
        if !fresh && !stop_running() {
            return Err("the installed copy is running and did not close".into());
        }
        // Read and write rather than copy: a copy would keep the download's "came from the
        // internet" mark, and Windows would warn about the installed exe every time it starts.
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let bytes = fs::read(&me).map_err(|e| format!("cannot read {}: {e}", me.display()))?;
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        // A copy we just closed can hold its exe for a moment after it exits: retry briefly.
        let mut attempt = 0;
        while let Err(e) = fs::write(&exe, &bytes) {
            attempt += 1;
            if attempt == 20 {
                return Err(format!("cannot write {}: {e}", exe.display()));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        register(&exe);
        if let Err(e) = create_shortcut(&exe) {
            log::info(format!("Could not create the Start menu shortcut: {e}"));
        }
        // Start with Windows: carry over an entry from a copy that ran in place (often in
        // Downloads); otherwise turn it on, unless LatencyLock has run here before with it off.
        let target = exe.to_string_lossy();
        if settings::has_autostart_entry() {
            settings::move_autostart(&target);
        } else if !used_before {
            settings::enable_autostart(&target);
        }
        log::info(format!(
            "Installed LatencyLock {VERSION} to {}",
            dir.display()
        ));
    } else if !updated && is_running() {
        message(HWND::default(), ALREADY_RUNNING, MB_ICONINFORMATION);
        return Ok(());
    }

    let mut installed_copy = Command::new(&exe);
    installed_copy.arg("--installed");
    if fresh {
        installed_copy.arg("--first-run");
    }
    if updated {
        installed_copy.arg("--updated");
    }
    installed_copy
        .spawn()
        .map(drop)
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

/// Write the Apps & features entry for `exe`. The installed copy also calls this at every start
/// (with its own path), so the version shown there follows updates.
pub fn register(exe: &Path) {
    let quoted = format!("\"{}\"", exe.display());
    let folder = exe.parent().unwrap_or(exe).display().to_string();
    let publisher = REPOSITORY
        .trim_start_matches("https://github.com/")
        .split('/')
        .next()
        .unwrap_or_default();
    for (name, value) in [
        ("DisplayName", "LatencyLock"),
        ("DisplayVersion", VERSION),
        ("Publisher", publisher),
        ("DisplayIcon", &exe.display().to_string()),
        ("UninstallString", &format!("{quoted} --uninstall")),
        ("InstallLocation", &folder),
        ("URLInfoAbout", REPOSITORY),
    ] {
        settings::set_string(UNINSTALL_KEY, name, value);
    }
    let size_kb = fs::metadata(exe).map_or(0, |m| (m.len() / 1024) as u32);
    for (name, value) in [("NoModify", 1), ("NoRepair", 1), ("EstimatedSize", size_kb)] {
        settings::set_dword(UNINSTALL_KEY, name, value);
    }
}

fn create_shortcut(exe: &Path) -> windows::core::Result<()> {
    let Some(lnk) = shortcut() else {
        return Ok(());
    };
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(w!(
            "Keeps Windows audio at the lowest buffer your driver supports"
        ))?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(lnk.as_os_str()), TRUE)
    }
}

fn is_running() -> bool {
    unsafe {
        OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, FALSE, MUTEX)
            .map(|m| {
                let _ = CloseHandle(m);
            })
            .is_ok()
    }
}

/// Ask a running LatencyLock (any version) to exit and wait until it has. True when none is
/// running any more.
fn stop_running() -> bool {
    unsafe {
        let Ok(mutex) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, FALSE, MUTEX) else {
            return true;
        };
        if let Ok(hwnd) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let r = WaitForSingleObject(mutex, 10_000);
        let stopped = r == WAIT_OBJECT_0 || r == WAIT_ABANDONED;
        if stopped {
            let _ = ReleaseMutex(mutex);
        }
        let _ = CloseHandle(mutex);
        stopped
    }
}

/// `--uninstall` (run by Apps & features): remove the app, its shortcut, Apps & features
/// entry, settings, Start with Windows entry and log.
pub fn uninstall() {
    let ask = "Uninstall LatencyLock?\n\n\
               This removes the app, its Start menu shortcut, its settings and its log.";
    if message(HWND::default(), ask, MB_YESNO | MB_ICONINFORMATION) != IDYES {
        return;
    }
    if !stop_running() {
        message(
            HWND::default(),
            "LatencyLock is still running and did not close. Exit it from the tray menu, \
             then try again.",
            MB_ICONWARNING,
        );
        return;
    }
    settings::remove_all();
    settings::delete_key(UNINSTALL_KEY);
    if let Some(lnk) = shortcut() {
        let _ = fs::remove_file(lnk);
    }
    if let Some(logs) = log::path().parent().filter(|p| p.is_absolute()) {
        let _ = fs::remove_dir_all(logs);
    }
    message(
        HWND::default(),
        "LatencyLock has been uninstalled.",
        MB_ICONINFORMATION,
    );

    let Some(dir) = install_dir().filter(|d| d.exists()) else {
        return;
    };
    if is_installed_copy() {
        // A running exe can't delete itself: have cmd remove the folder once this has exited.
        let _ = Command::new("cmd.exe")
            .raw_arg(format!(
                "/d /c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
                dir.display()
            ))
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else {
        let _ = fs::remove_dir_all(&dir);
    }
}
