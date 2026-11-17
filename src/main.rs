#![cfg_attr(not(test), windows_subsystem = "windows")]
//! LatencyLock: keeps Windows' shared-mode audio engine at the smallest buffer the driver
//! supports, and puts it back automatically whenever a device reset undoes it.

mod audio;
mod log;
mod settings;
mod tray;
mod update;

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WAIT_ABANDONED,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, HMENU, IDYES, MB_ICONINFORMATION, MB_ICONWARNING,
    MB_OK, MB_YESNO, MENU_ITEM_FLAGS, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MF_CHECKED, MF_DISABLED,
    MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SW_SHOWNORMAL, SetForegroundWindow, SetTimer, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};
use windows::core::{HSTRING, PCWSTR, w};

use audio::{DeviceStatus, Engine, Mode, State};
use tray::{Look, Tray, WM_TRAY};
use update::{Outcome, WM_UPDATE};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TIMER_ID: usize = 1;
const TICK_MS: u32 = 1000;
const TIMER_UPDATE: usize = 2;
/// First update check a minute after start (the network may not be up yet at login), then
/// every 12 hours, or an hour after a check that failed.
const UPDATE_FIRST_MS: u32 = 60 * 1000;
const UPDATE_EVERY_MS: u32 = 12 * 60 * 60 * 1000;
const UPDATE_RETRY_MS: u32 = 60 * 60 * 1000;

const CMD_MODE_DEFAULT: usize = 10;
const CMD_MODE_ALL: usize = 11;
const CMD_PAUSE: usize = 12;
const CMD_AUTOSTART: usize = 13;
const CMD_DETAILS: usize = 14;
const CMD_LOG: usize = 15;
const CMD_EXIT: usize = 16;
const CMD_AUTO_UPDATE: usize = 17;
const CMD_CHECK_UPDATE: usize = 18;

const HELP: &str = "LatencyLock - keeps Windows audio at the lowest buffer your driver supports.

Runs in the notification area. Right-click the headphones icon for status and settings.

  latency-lock.exe            start the tray app
  latency-lock.exe --probe    print every output device's buffer limits and exit
  latency-lock.exe --help     show this help";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` on the app state. Uses try_borrow so re-entrant messages (COM calls and modal
/// menus pump messages) are skipped instead of panicking.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut a| a.as_mut().map(f))
    })
}

struct App {
    engine: Engine,
    tray: Tray,
    mode: Mode,
    paused: bool,
    statuses: Vec<DeviceStatus>,
}

fn short(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_owned()
    } else {
        format!("{}...", name.chars().take(max - 3).collect::<String>())
    }
}

impl App {
    fn tick(&mut self) {
        self.statuses = self.engine.tick(self.mode, self.paused);
        let (look, tip) = self.summary();
        self.tray.update(look, &tip);
    }

    fn summary(&self) -> (Look, String) {
        if self.paused {
            return (
                Look::Paused,
                "LatencyLock: paused (Windows default buffers)".into(),
            );
        }
        if self.statuses.is_empty() {
            return (Look::Idle, "LatencyLock: no output device".into());
        }
        let look = if self
            .statuses
            .iter()
            .any(|s| matches!(s.state, State::Failed(_) | State::LockedByOther))
        {
            Look::Warning
        } else if self.statuses.iter().any(|s| s.state == State::Locked) {
            Look::Locked
        } else {
            Look::Idle
        };
        let s = self
            .statuses
            .iter()
            .find(|s| s.is_default)
            .unwrap_or(&self.statuses[0]);
        let what = match &s.state {
            State::Locked => format!("{:.2} ms (locked)", s.ms(s.current)),
            State::NothingToLock => format!("{:.2} ms (driver minimum)", s.ms(s.current)),
            State::LockedByOther => format!("{:.2} ms (set by another app)", s.ms(s.current)),
            State::Failed(_) => "could not lock".into(),
        };
        let mut tip = format!("LatencyLock: {what}\n{}", short(&s.name, 60));
        if self.statuses.len() > 1 {
            tip.push_str(&format!("\n+{} more output(s)", self.statuses.len() - 1));
        }
        (look, tip)
    }

    fn build_menu(&self) -> HMENU {
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
            };
            let info = MF_STRING | MF_DISABLED; // not clickable, but not greyed out

            if self.paused {
                add(info, 0, "Paused - Windows default buffers in use");
            } else if self.statuses.is_empty() {
                add(info, 0, "No output device");
            }
            for s in &self.statuses {
                let value = match &s.state {
                    State::Locked => format!("{:.2} ms  locked", s.ms(s.current)),
                    State::NothingToLock => format!("{:.2} ms  no lower buffer", s.ms(s.current)),
                    State::LockedByOther => {
                        format!("{:.2} ms  set by another app", s.ms(s.current))
                    }
                    State::Failed(_) => "error - see log".into(),
                };
                add(info, 0, &format!("{}\t{value}", short(&s.name, 44)));
            }
            if self.engine.recoveries > 0 {
                let n = self.engine.recoveries;
                add(
                    info,
                    0,
                    &format!(
                        "Re-applied after {n} device reset{}",
                        if n == 1 { "" } else { "s" }
                    ),
                );
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            let check = |on: bool| {
                if on {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                }
            };
            add(
                check(self.mode == Mode::DefaultOutput),
                CMD_MODE_DEFAULT,
                "Default output only",
            );
            add(
                check(self.mode == Mode::AllOutputs),
                CMD_MODE_ALL,
                "All outputs",
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            add(check(self.paused), CMD_PAUSE, "Pause");
            add(
                check(settings::autostart_enabled()),
                CMD_AUTOSTART,
                "Start with Windows",
            );
            add(
                check(auto_update()),
                CMD_AUTO_UPDATE,
                "Install updates automatically",
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_DETAILS, "Device details...");
            add(MF_STRING, CMD_LOG, "Open log");
            add(MF_STRING, CMD_CHECK_UPDATE, "Check for updates...");
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_EXIT, "Exit");
            menu
        }
    }

    fn command(&mut self, cmd: usize) {
        match cmd {
            CMD_MODE_DEFAULT | CMD_MODE_ALL => {
                self.mode = if cmd == CMD_MODE_ALL {
                    Mode::AllOutputs
                } else {
                    Mode::DefaultOutput
                };
                settings::set_flag(settings::ALL_OUTPUTS, self.mode == Mode::AllOutputs);
                log::info(format!("Mode: {:?}", self.mode));
            }
            CMD_PAUSE => {
                self.paused = !self.paused;
                if !self.paused {
                    log::info("Resumed");
                }
            }
            CMD_AUTOSTART => {
                let on = !settings::autostart_enabled();
                settings::set_autostart(on);
                log::info(format!(
                    "Start with Windows: {}",
                    if on { "on" } else { "off" }
                ));
            }
            CMD_AUTO_UPDATE => {
                let on = !auto_update();
                settings::set_flag(settings::AUTO_UPDATE, on);
                log::info(format!(
                    "Install updates automatically: {}",
                    if on { "on" } else { "off" }
                ));
            }
            _ => return,
        }
        self.tick();
    }

    fn details(&mut self) -> String {
        let mut text = audio::describe(&self.engine.probe());
        text.push_str(&format!(
            "\n\nLatencyLock {VERSION} checks the hold every second and re-applies it after \
             device resets. If you hear crackling, the buffer is too small for your system: \
             use Pause, or the device's own driver/settings."
        ));
        text
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == TIMER_UPDATE => {
                SetTimer(hwnd, TIMER_UPDATE, UPDATE_EVERY_MS, None);
                // Installing restarts the app, which would lift a pause; wait until resumed.
                let paused = with_app(|a| a.paused).unwrap_or(true);
                if auto_update() && !paused {
                    update::start(hwnd, false, true);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                with_app(|a| a.tick());
                LRESULT(0)
            }
            WM_UPDATE => {
                on_update(hwnd, update::take(lp));
                LRESULT(0)
            }
            WM_TRAY => {
                let event = (lp.0 & 0xFFFF) as u32;
                if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                    show_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                with_app(|a| a.tray.readd());
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn show_menu(hwnd: HWND) {
    // Build the menu, then release the app borrow: TrackPopupMenu runs a modal loop that
    // keeps delivering WM_TIMER, and the ticks must keep working while the menu is open.
    let Some(menu) = with_app(|a| a.build_menu()) else {
        return;
    };
    let cmd = unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    };
    match cmd {
        0 => {}
        CMD_EXIT => unsafe {
            log::info("Exit requested");
            let _ = DestroyWindow(hwnd);
        },
        CMD_DETAILS => {
            if let Some(text) = with_app(|a| a.details()) {
                unsafe {
                    MessageBoxW(
                        hwnd,
                        &HSTRING::from(text),
                        w!("LatencyLock - device details"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
        }
        CMD_CHECK_UPDATE => {
            if !update::start(hwnd, true, false) {
                message(
                    hwnd,
                    "An update check is already running.",
                    MB_ICONINFORMATION,
                );
            }
        }
        CMD_LOG => unsafe {
            ShellExecuteW(
                hwnd,
                w!("open"),
                &HSTRING::from(log::path().as_os_str()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        },
        other => {
            with_app(|a| a.command(other));
        }
    }
}

fn auto_update() -> bool {
    settings::get_flag_or(settings::AUTO_UPDATE, true)
}

fn message(hwnd: HWND, text: &str, icon: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(hwnd, &HSTRING::from(text), w!("LatencyLock"), icon) }
}

/// A finished update check (see update.rs). Automatic checks stay quiet unless they install
/// something; checks started from the menu always answer.
fn on_update(hwnd: HWND, report: update::Report) {
    match report.outcome {
        Outcome::Installed(v) => {
            log::info(format!("Updated to {v}; restarting"));
            match update::launch_new() {
                Ok(()) => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                Err(e) => {
                    log::info(format!("Could not start the new version: {e}"));
                    if report.manual {
                        message(
                            hwnd,
                            &format!(
                                "LatencyLock {v} is installed. Restart LatencyLock to use it."
                            ),
                            MB_ICONINFORMATION,
                        );
                    }
                }
            }
        }
        Outcome::Available(v) => {
            let ask = format!(
                "LatencyLock {v} is available (you have {VERSION}).\n\nInstall it and restart now?"
            );
            if message(hwnd, &ask, MB_YESNO | MB_ICONINFORMATION) == IDYES
                && !update::start(hwnd, true, true)
            {
                message(
                    hwnd,
                    "An update check is already running.",
                    MB_ICONINFORMATION,
                );
            }
        }
        Outcome::UpToDate(latest) if report.manual => {
            message(
                hwnd,
                &format!(
                    "You are up to date.\n\nThis version: {VERSION}\nLatest release: {latest}"
                ),
                MB_ICONINFORMATION,
            );
        }
        Outcome::UpToDate(_) => {}
        Outcome::Failed(e) => {
            log::info(format!("Update check failed: {e}"));
            if report.manual {
                message(hwnd, &format!("Could not update:\n{e}"), MB_ICONWARNING);
            } else {
                unsafe { SetTimer(hwnd, TIMER_UPDATE, UPDATE_RETRY_MS, None) };
            }
        }
    }
}

/// `--probe`: print every output's buffer limits to the console we were started from.
fn probe() {
    unsafe {
        // GUI-subsystem exe: borrow the parent console unless output is already redirected.
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let report = audio::describe(&Engine::new().probe());
    let mut out = std::io::stdout();
    let _ = writeln!(out, "\n{report}");
    let _ = out.flush();
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        probe();
        return;
    }
    // Started by the previous version right after it installed an update.
    let updated = args == ["--updated"];
    if !args.is_empty() && !updated {
        unsafe {
            MessageBoxW(
                HWND::default(),
                &HSTRING::from(HELP),
                w!("LatencyLock"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        return;
    }

    // One instance per user session.
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\LatencyLock.SingleInstance"));
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        // After an update the old version is still shutting down: wait for it to let go.
        let took_over = updated
            && exists
            && mutex.as_ref().is_ok_and(|&m| {
                let r = WaitForSingleObject(m, 15_000);
                r == WAIT_OBJECT_0 || r == WAIT_ABANDONED
            });
        if mutex.is_err() || (exists && !took_over) {
            MessageBoxW(
                HWND::default(),
                w!(
                    "LatencyLock is already running. Look for the headphones icon in the notification area."
                ),
                w!("LatencyLock"),
                MB_OK | MB_ICONINFORMATION,
            );
            return;
        }
        // The handle lives until the process exits.
    }

    log::init();
    let mode = if settings::get_flag(settings::ALL_OUTPUTS) {
        Mode::AllOutputs
    } else {
        Mode::DefaultOutput
    };
    log::info(format!("LatencyLock {VERSION} started (mode: {mode:?})"));
    update::cleanup();

    let hwnd = unsafe {
        let instance: HINSTANCE = GetModuleHandleW(PCWSTR::null()).unwrap_or_default().into();
        let class = w!("LatencyLockWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
        // A hidden top-level window (not message-only) so it receives TaskbarCreated.
        match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("LatencyLock"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log::info(format!("Could not create window: {e}"));
                return;
            }
        }
    };

    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            engine: Engine::new(),
            tray: Tray::new(hwnd),
            mode,
            paused: false,
            statuses: Vec::new(),
        })
    });
    with_app(|a| a.tick());

    unsafe {
        SetTimer(hwnd, TIMER_ID, TICK_MS, None);
        SetTimer(hwnd, TIMER_UPDATE, UPDATE_FIRST_MS, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Dropping the app stops our streams (Windows returns to default buffers) and removes the icon.
    APP.with(|cell| cell.borrow_mut().take());
    log::info("LatencyLock stopped");
}
