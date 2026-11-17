//! Notification-area icon. The headphones glyph (glyph.rs) is drawn in code and tinted by state,
//! so the icon alone tells you whether the low buffer is being held.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Foundation::{HANDLE, HWND, TRUE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HDC,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON, WM_APP,
};

use crate::glyph;

pub const WM_TRAY: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    /// Holding the minimum buffer.
    Locked,
    /// Nothing to do (no device, or the driver has no smaller buffer).
    Idle,
    /// Something prevents the lock (error, or another app fixed the period).
    Warning,
    Paused,
}

pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 4],
    shown: Option<(Look, String)>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
        Self {
            hwnd,
            icons: [
                make_icon(size, (46, 160, 67), false),   // Locked: green
                make_icon(size, (139, 148, 158), false), // Idle: grey
                make_icon(size, (210, 153, 34), false),  // Warning: amber
                make_icon(size, (139, 148, 158), true),  // Paused: grey, headband lifted
            ],
            shown: None,
        }
    }

    fn data(&self, look: Look, tip: &str) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icons[look as usize],
            ..Default::default()
        };
        for (dst, src) in d.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *dst = src;
        }
        d
    }

    /// Show `look`/`tip`; only talks to the shell when something changed.
    pub fn update(&mut self, look: Look, tip: &str) {
        if let Some((l, t)) = &self.shown
            && *l == look
            && t == tip
        {
            return;
        }
        let d = self.data(look, tip);
        unsafe {
            let ok = if self.shown.is_some() {
                // MODIFY fails if Explorer lost the icon; fall back to ADD.
                Shell_NotifyIconW(NIM_MODIFY, &d).as_bool()
                    || Shell_NotifyIconW(NIM_ADD, &d).as_bool()
            } else {
                Shell_NotifyIconW(NIM_ADD, &d).as_bool()
            };
            self.shown = ok.then(|| (look, tip.to_owned()));
        }
    }

    /// A one-off notification from the icon (shown as a toast on Windows 10 and 11).
    pub fn notify(&self, title: &str, text: &str) {
        if self.shown.is_none() {
            return;
        }
        let mut d = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_INFO | NIIF_NOSOUND,
            ..Default::default()
        };
        for (dst, src) in d.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
            *dst = src;
        }
        for (dst, src) in d.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
            *dst = src;
        }
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Explorer restarted (TaskbarCreated): add the icon again.
    pub fn readd(&mut self) {
        if let Some((look, tip)) = self.shown.take() {
            self.update(look, &tip);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let d = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
            for icon in self.icons {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

/// Render the headphones into a 32-bit ARGB icon.
fn make_icon(size: i32, (r, g, b): (u8, u8, u8), open: bool) -> HICON {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        let Ok(color) = CreateDIBSection(
            HDC::default(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE::default(),
            0,
        ) else {
            return HICON::default();
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        let rgb = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        for (p, &a) in px.iter_mut().zip(&glyph::coverage(size as usize, open)) {
            *p = if a == 0 { 0 } else { ((a as u32) << 24) | rgb };
        }
        // Monochrome mask is required by CreateIconIndirect but ignored for 32-bit alpha icons.
        let mask_bits = vec![0u8; (((size + 15) / 16) * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let info = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}
