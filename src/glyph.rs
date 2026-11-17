//! The headphones glyph: the headband is a padlock shackle, with a keyhole between the earcups.
//! Drawn in code and shared by the tray icon (tray.rs) and the exe's own icon (build.rs), so the
//! two always match.

/// Is the point (u, v) in unit square coordinates (v grows downward) inside the headphones?
/// The headband is a padlock shackle and a keyhole sits between the earcups.
pub fn headphones(u: f32, v: f32, open: bool) -> bool {
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

/// Coverage of each pixel of a `size` x `size` render (0-255, rows top to bottom), from 4x4
/// samples per pixel for smooth edges.
pub fn coverage(size: usize, open: bool) -> Vec<u8> {
    const SS: usize = 4;
    let mut out = vec![0u8; size * size];
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    hits += headphones(u, v, open) as usize;
                }
            }
            out[y * size + x] = (hits * 255 / (SS * SS)) as u8;
        }
    }
    out
}
