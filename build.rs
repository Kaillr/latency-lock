//! Gives latency-lock.exe its icon (the tray icon's headphones glyph, drawn by src/glyph.rs) and
//! version info, which Explorer, the Start menu, Apps & features and Task Manager show.
//!
//! The icon file is always generated. Compiling it into the exe needs the Windows SDK's rc.exe,
//! so that step runs only when building on Windows (cross-checking from another OS skips it).

#[path = "src/glyph.rs"]
mod glyph;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Icon sizes Windows picks from (16-64 px covers 100-200% display scaling for the places the
/// icon appears). Larger views scale the 64 px image up.
const SIZES: [usize; 7] = [16, 20, 24, 32, 40, 48, 64];
/// The tray icon's "holding the low buffer" green.
const COLOR: [u8; 3] = [22, 163, 74];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/glyph.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("latency-lock.ico");
    fs::write(&ico, icon()).expect("write icon");

    let for_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !for_windows || !cfg!(windows) {
        return;
    }
    let rc = out.join("latency-lock.rc");
    fs::write(&rc, resource_script(&ico)).expect("write resource script");
    // Required, so a release can't quietly ship without its icon and version info.
    if let Err(e) = embed_resource::compile(&rc, embed_resource::NONE).manifest_required() {
        panic!("could not compile Windows resources: {e}");
    }
}

/// An .ico file: a directory of entries, then one 32-bit image per size.
fn icon() -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&size| bitmap(size)).collect();
    let mut ico = Vec::new();
    for v in [0u16, 1, images.len() as u16] {
        ico.extend(v.to_le_bytes()); // reserved, type 1 = icon, count
    }
    let mut offset = 6 + 16 * images.len();
    for (&size, image) in SIZES.iter().zip(&images) {
        ico.extend([size as u8, size as u8, 0, 0]); // width, height, palette size, reserved
        ico.extend(1u16.to_le_bytes()); // planes
        ico.extend(32u16.to_le_bytes()); // bits per pixel
        ico.extend((image.len() as u32).to_le_bytes());
        ico.extend((offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        ico.extend(image);
    }
    ico
}

/// One icon image: a BITMAPINFOHEADER, bottom-up BGRA pixels, then the 1-bit AND mask that
/// icons still carry (set where the pixel is fully transparent).
fn bitmap(size: usize) -> Vec<u8> {
    let alpha = glyph::coverage(size, false);
    let mask_row = size.div_ceil(32) * 4;
    let mut d = Vec::new();
    // The header's height counts the pixel rows and the mask rows.
    for v in [40u32, size as u32, 2 * size as u32] {
        d.extend(v.to_le_bytes());
    }
    d.extend(1u16.to_le_bytes()); // planes
    d.extend(32u16.to_le_bytes()); // bits per pixel
    d.extend(0u32.to_le_bytes()); // BI_RGB
    d.extend(((size * size * 4 + mask_row * size) as u32).to_le_bytes());
    d.extend([0u8; 16]); // resolution and palette fields, unused
    for y in (0..size).rev() {
        for x in 0..size {
            d.extend([COLOR[2], COLOR[1], COLOR[0], alpha[y * size + x]]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..size {
            if alpha[y * size + x] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        d.extend(row);
    }
    d
}

fn resource_script(ico: &Path) -> String {
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let numbers: Vec<&str> = version.split(['.', '-', '+']).take(3).collect();
    let numeric = format!("{},0", numbers.join(","));
    let repository = env::var("CARGO_PKG_REPOSITORY").unwrap_or_default();
    let owner = repository
        .trim_start_matches("https://github.com/")
        .split('/')
        .next()
        .unwrap_or_default();
    let license = env::var("CARGO_PKG_LICENSE").unwrap_or_default();
    let ico = ico.display().to_string().replace('\\', "\\\\");
    format!(
        r#"1 ICON "{ico}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{owner}"
      VALUE "FileDescription", "LatencyLock"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "latency-lock"
      VALUE "LegalCopyright", "Copyright (c) {owner}. {license} License."
      VALUE "OriginalFilename", "latency-lock.exe"
      VALUE "ProductName", "LatencyLock"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
