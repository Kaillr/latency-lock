//! Persistent settings (HKCU\Software\LatencyLock) and the "Start with Windows" entry.

use windows::Win32::Foundation::{ERROR_SUCCESS, HMODULE};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{PCWSTR, w};

const SETTINGS_KEY: PCWSTR = w!("Software\\LatencyLock");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APPROVED_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
const RUN_VALUE: PCWSTR = w!("LatencyLock");
pub const ALL_OUTPUTS: PCWSTR = w!("AllOutputs");

pub fn get_flag(name: PCWSTR) -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && data != 0
}

pub fn set_flag(name: PCWSTR, on: bool) {
    let v: u32 = on as u32;
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            SETTINGS_KEY,
            name,
            REG_DWORD.0,
            Some(&v as *const u32 as _),
            4,
        );
    }
}

fn get_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn get_first_byte(key: PCWSTR, name: PCWSTR) -> Option<u8> {
    let mut buf = [0u8; 64];
    let mut size = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS && size > 0).then_some(buf[0])
}

pub fn exe_path() -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(HMODULE::default(), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// True when the Run entry points at this exe and Task Manager hasn't disabled it.
pub fn autostart_enabled() -> bool {
    let Some(cmd) = get_string(RUN_KEY, RUN_VALUE) else {
        return false;
    };
    if !cmd.to_lowercase().contains(&exe_path().to_lowercase()) {
        return false;
    }
    // StartupApproved: first byte even = enabled, odd = disabled in Task Manager.
    get_first_byte(APPROVED_KEY, RUN_VALUE).is_none_or(|b| b & 1 == 0)
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let cmd: Vec<u16> = format!("\"{}\"", exe_path())
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as _),
                (cmd.len() * 2) as u32,
            );
            // Clear a "disabled" mark left by Task Manager so the entry really runs.
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VALUE);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}
