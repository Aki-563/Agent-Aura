//! The mouse cursor while the agent is working: an arrow pointer like the normal one, but filled
//! with Aura's violet-to-cyan gradient (white rim, soft shadow), so the person can tell at a
//! glance that Aura, not they, is moving it.
//!
//! The system's default arrow, text and hand cursors are replaced for the length of a run with
//! `SetSystemCursor` and put back with `SystemParametersInfo(SPI_SETCURSORS)`, which reloads the
//! user's own cursor scheme. `restore()` is called when a run ends, when Aura starts (in case a
//! previous run was killed mid-way), and on drop of the guard.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CopyIcon, CreateIconIndirect, DestroyIcon, GetSystemMetrics, SetSystemCursor, SystemParametersInfoW, ICONINFO, OCR_HAND, OCR_IBEAM, OCR_NORMAL,
    SM_CXCURSOR, SPIF_SENDCHANGE, SPI_SETCURSORS,
};

static REPLACED: AtomicBool = AtomicBool::new(false);

/// The arrow's outline in a 32 x 32 design grid; the tip (the hotspot) is the first point.
const ARROW: [(f32, f32); 7] = [(3.0, 2.0), (3.0, 25.0), (8.6, 19.8), (12.4, 28.4), (16.2, 26.8), (12.5, 18.4), (19.6, 18.4)];

/// Hotspot of a cursor `size` pixels wide: the arrow's tip.
pub fn hotspot(size: i32) -> (u32, u32) {
    let s = size as f32 / 32.0;
    ((ARROW[0].0 * s).round() as u32, (ARROW[0].1 * s).round() as u32)
}

/// Signed distance (negative inside) from (x, y) to the arrow polygon scaled by `s`.
fn arrow_distance(x: f32, y: f32, s: f32) -> f32 {
    let points: Vec<(f32, f32)> = ARROW.iter().map(|(px, py)| (px * s, py * s)).collect();
    let mut inside = false;
    let mut nearest = f32::MAX;
    for i in 0..points.len() {
        let (ax, ay) = points[i];
        let (bx, by) = points[(i + 1) % points.len()];
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        nearest = nearest.min((x - ax - t * dx).hypot(y - ay - t * dy));
    }
    if inside { -nearest } else { nearest }
}

/// Straight-alpha RGBA pixels (row by row) of the working pointer, `size` x `size`.
pub fn arrow_pixels(size: i32) -> Vec<[u8; 4]> {
    let s = size as f32 / 32.0;
    let rim = 1.5 * s;
    let (top, bottom) = ([139.0f32, 92.0, 246.0], [34.0f32, 211.0, 238.0]);
    (0..size * size)
        .map(|i| {
            let (x, y) = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
            let d = arrow_distance(x, y, s);
            let shape = (0.5 - d).clamp(0.0, 1.0);
            let fill = (0.5 - (d + rim)).clamp(0.0, 1.0);
            let t = ((y / s - 2.0) / 26.0).clamp(0.0, 1.0);
            let body = [0, 1, 2].map(|c| top[c] + (bottom[c] - top[c]) * t);
            let colour = [0, 1, 2].map(|c| 255.0 + (body[c] - 255.0) * fill);
            // Soft shadow down and to the right, under the arrow.
            let sd = arrow_distance(x - 1.2 * s, y - 1.8 * s, s);
            let shadow = 0.30 * ((1.6 * s - sd) / (3.2 * s)).clamp(0.0, 1.0);
            let alpha = shape + shadow * (1.0 - shape);
            if alpha <= 0.0 {
                return [0, 0, 0, 0];
            }
            let mix = |c: f32| (c * shape / alpha).round().clamp(0.0, 255.0) as u8;
            [mix(colour[0]), mix(colour[1]), mix(colour[2]), (alpha * 255.0).round() as u8]
        })
        .collect()
}

/// The cursor size for the main monitor's *current* scale. `GetSystemMetrics` answers for the scale
/// Windows had at sign-in, so after the user changes the scale it would be too small or too big.
fn cursor_size() -> i32 {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI};
    let size = unsafe {
        let (mut dx, mut dy) = (0u32, 0u32);
        let monitor = MonitorFromPoint(windows_sys::Win32::Foundation::POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) == 0 && dx > 0 {
            GetSystemMetricsForDpi(SM_CXCURSOR, dx)
        } else {
            GetSystemMetrics(SM_CXCURSOR)
        }
    };
    if (16..=256).contains(&size) { size } else { 32 }
}

/// Builds the cursor handle (hotspot in the middle). The caller owns it.
fn build_cursor() -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    let size = cursor_size();
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB as u32,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return None;
        }
        // 32-bit BGRA, premultiplied is what the cursor compositor expects with a 32-bit bitmap.
        let target = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        for (slot, [r, g, b, a]) in target.iter_mut().zip(arrow_pixels(size)) {
            let premultiply = |channel: u8| (channel as u32 * a as u32 / 255) & 0xFF;
            *slot = ((a as u32) << 24) | (premultiply(r) << 16) | (premultiply(g) << 8) | premultiply(b);
        }
        // The mask is ignored for 32-bit colour bitmaps but must exist.
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let (hot_x, hot_y) = hotspot(size);
        let icon_info = ICONINFO { fIcon: 0, xHotspot: hot_x, yHotspot: hot_y, hbmMask: mask, hbmColor: color };
        let cursor = CreateIconIndirect(&icon_info);
        DeleteObject(color);
        DeleteObject(mask);
        (!cursor.is_null()).then_some(cursor)
    }
}

/// Switches the arrow, text and hand cursors to the working pointer. Safe to call twice.
pub fn show_working_cursor() {
    if cfg!(test) || std::env::var_os("AURA_NO_CURSOR").is_some() || REPLACED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(cursor) = build_cursor() else {
        REPLACED.store(false, Ordering::SeqCst);
        return;
    };
    crate::recovery::note_cursor(true);
    unsafe {
        for id in [OCR_NORMAL, OCR_IBEAM, OCR_HAND] {
            // SetSystemCursor takes ownership of the handle it is given, so each gets a copy.
            let copy = CopyIcon(cursor);
            if !copy.is_null() {
                SetSystemCursor(copy, id);
            }
        }
        DestroyIcon(cursor);
    }
}

/// Puts the user's own cursors back.
pub fn restore() {
    if cfg!(test) {
        return;
    }
    REPLACED.store(false, Ordering::SeqCst);
    unsafe {
        SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), SPIF_SENDCHANGE);
    }
    crate::recovery::note_cursor(false);
}

/// Restores the cursors when dropped, so an early return or panic cannot leave the working pointer behind.
pub struct WorkingCursor;

impl WorkingCursor {
    pub fn show() -> Self {
        show_working_cursor();
        Self
    }
}

impl Drop for WorkingCursor {
    fn drop(&mut self) {
        restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_is_an_arrow_with_its_tip_at_the_hotspot() {
        for size in [32, 48, 64] {
            let pixels = arrow_pixels(size);
            assert_eq!(pixels.len(), (size * size) as usize);
            let at = |x: i32, y: i32| pixels[(y * size + x) as usize];
            let s = size as f32 / 32.0;
            let (hx, hy) = hotspot(size);
            assert!(at(hx as i32, hy as i32 + 1)[3] > 120, "the tip is drawn at the hotspot ({size})");
            assert_eq!(at(size - 1, 0)[3], 0, "the top-right corner is clear");
            // Inside the body: the gradient fill (bluish violet, not white).
            let body = at((6.0 * s) as i32, (14.0 * s) as i32);
            assert!(body[3] > 240 && body[2] > body[1], "body {body:?}");
            // The rim is white.
            let rim = at((3.4 * s) as i32, (12.0 * s) as i32);
            assert!(rim[0] > 200 && rim[1] > 200 && rim[2] > 200, "rim {rim:?}");
        }
    }

    #[test]
    fn show_and_restore_do_nothing_under_test() {
        show_working_cursor();
        restore();
        let _guard = WorkingCursor::show();
    }
}
