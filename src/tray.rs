//! Notification-area icon. A headphones glyph (the headband is a padlock shackle, with a keyhole
//! between the earcups) is drawn in code (no image assets) and tinted by state, so the icon alone
//! tells you whether the low buffer is being held.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Foundation::{HANDLE, HWND, TRUE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HDC,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON, WM_APP,
};

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

/// Is the point (u, v) in unit square coordinates (v grows downward) inside the headphones?
/// The headband is a padlock shackle and a keyhole sits between the earcups.
fn headphones(u: f32, v: f32, open: bool) -> bool {
    // Headband: a shackle (upper half of a ring plus two legs). When open it is lifted clear
    // and its right leg stops short.
    let lift = if open { 0.13 } else { 0.0 };
    let (cx, cy, ro, ri) = (0.5, 0.50 - lift, 0.37, 0.25);
    // Earcups: rounded rectangles, mirrored about the centre line. They sit mostly outside the
    // headband, reaching just past its inner edge.
    let m = 0.5 - (u - 0.5).abs();
    let (x1, y0, y1, w, r) = (cx - ri + 0.03, 0.52, 0.88, 0.24, 0.08);
    let x0 = x1 - w;
    if (x0..=x1).contains(&m) && (y0..=y1).contains(&v) {
        let dx = m - m.clamp(x0 + r, x1 - r);
        let dy = v - v.clamp(y0 + r, y1 - r);
        if dx * dx + dy * dy <= r * r {
            return true;
        }
    }
    // Keyhole: a round head over a stem that flares toward the bottom.
    let (kx, ky) = (u - 0.5, v - 0.62);
    if kx * kx + ky * ky <= 0.095 * 0.095 {
        return true;
    }
    if (0.62..=0.86).contains(&v) && kx.abs() <= 0.035 + (v - 0.62) * 0.18 {
        return true;
    }
    let (dx, dy) = (u - cx, v - cy);
    let d2 = dx * dx + dy * dy;
    if v <= cy {
        return d2 <= ro * ro && d2 >= ri * ri;
    }
    let left = (cx - ro..=cx - ri).contains(&u);
    let right = (cx + ri..=cx + ro).contains(&u);
    let leg_end = if open && right { cy + 0.02 } else { 0.60 };
    (left || right) && v <= leg_end
}

/// Render the headphones into a 32-bit ARGB icon with 4x4 supersampled edges.
fn make_icon(size: i32, (r, g, b): (u8, u8, u8), open: bool) -> HICON {
    const SS: i32 = 4;
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
        for y in 0..size {
            for x in 0..size {
                let mut hits = 0;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                        let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                        hits += headphones(u, v, open) as i32;
                    }
                }
                let a = (hits * 255 / (SS * SS)) as u32;
                px[(y * size + x) as usize] = if a == 0 {
                    0
                } else {
                    (a << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
                };
            }
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
