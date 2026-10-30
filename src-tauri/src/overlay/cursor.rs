//! The mouse cursor while the agent is working: a ring with a centre dot instead of the arrow, so
//! the person can tell at a glance that Aura (and not they) is moving the pointer.
//!
//! The system's default arrow, text and hand cursors are replaced for the length of a run with
//! `SetSystemCursor` and put back with `SystemParametersInfo(SPI_SETCURSORS)`, which reloads the
//! user's own cursor scheme. `restore()` is called when a run ends, when Aura starts (in case a
//! previous run was killed mid-way), and on drop of the guard.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CopyIcon, CreateIconIndirect, DestroyIcon, SetSystemCursor, SystemParametersInfoW, ICONINFO, OCR_HAND, OCR_IBEAM, OCR_NORMAL,
    SPIF_SENDCHANGE, SPI_SETCURSORS,
};

const SIZE: i32 = 32;
static REPLACED: AtomicBool = AtomicBool::new(false);

/// Straight-alpha RGBA pixels (row by row) of the working cursor: a cyan ring with a white
/// outline on both sides so it reads on light and dark backgrounds, and a centre dot.
pub fn ring_pixels() -> Vec<[u8; 4]> {
    let c = (SIZE as f32 - 1.0) / 2.0;
    (0..SIZE * SIZE)
        .map(|i| {
            let (x, y) = ((i % SIZE) as f32, (i / SIZE) as f32);
            let d = (x - c).hypot(y - c);
            // Coverage helpers: 1 inside [lo, hi], with a soft one pixel edge.
            let band = |lo: f32, hi: f32| ((d - lo + 0.5).clamp(0.0, 1.0) * (hi - d + 0.5).clamp(0.0, 1.0)).clamp(0.0, 1.0);
            let ring = band(7.0, 9.0);
            let outline = band(5.8, 10.2) * (1.0 - ring);
            let dot = band(0.0, 2.4);
            let dot_outline = band(0.0, 3.6) * (1.0 - dot);
            // Compose: white outline under the cyan ring and dot.
            let mut a = outline.max(dot_outline) * 0.92;
            let (mut r, mut g, mut b) = (255.0f32, 255.0, 255.0);
            for (cover, cr, cg, cb) in [(ring, 6.0, 182.0, 212.0), (dot, 6.0, 182.0, 212.0)] {
                r = r * (1.0 - cover) + cr * cover;
                g = g * (1.0 - cover) + cg * cover;
                b = b * (1.0 - cover) + cb * cover;
                a = a + (1.0 - a) * cover;
            }
            [r as u8, g as u8, b as u8, (a.clamp(0.0, 1.0) * 255.0) as u8]
        })
        .collect()
}

/// Builds the cursor handle (hotspot in the middle). The caller owns it.
fn build_cursor() -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            biHeight: -SIZE, // top-down
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
        let target = std::slice::from_raw_parts_mut(bits as *mut u32, (SIZE * SIZE) as usize);
        for (slot, [r, g, b, a]) in target.iter_mut().zip(ring_pixels()) {
            let premultiply = |channel: u8| (channel as u32 * a as u32 / 255) & 0xFF;
            *slot = ((a as u32) << 24) | (premultiply(r) << 16) | (premultiply(g) << 8) | premultiply(b);
        }
        // The mask is ignored for 32-bit colour bitmaps but must exist.
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, std::ptr::null());
        let icon_info = ICONINFO { fIcon: 0, xHotspot: (SIZE / 2) as u32, yHotspot: (SIZE / 2) as u32, hbmMask: mask, hbmColor: color };
        let cursor = CreateIconIndirect(&icon_info);
        DeleteObject(color);
        DeleteObject(mask);
        (!cursor.is_null()).then_some(cursor)
    }
}

/// Switches the arrow, text and hand cursors to the working ring. Safe to call twice.
pub fn show_working_cursor() {
    if cfg!(test) || std::env::var_os("AURA_NO_CURSOR").is_some() || REPLACED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(cursor) = build_cursor() else {
        REPLACED.store(false, Ordering::SeqCst);
        return;
    };
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
}

/// Restores the cursors when dropped, so an early return or panic cannot leave the ring behind.
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
    fn the_ring_is_a_transparent_square_with_a_visible_ring_and_dot() {
        let pixels = ring_pixels();
        assert_eq!(pixels.len(), (SIZE * SIZE) as usize);
        let at = |x: i32, y: i32| pixels[(y * SIZE + x) as usize];
        assert!(at(16, 16)[3] > 200, "the centre dot is solid");
        assert!(at(0, 0)[3] == 0, "the corners are transparent");
        assert!(at(16, 8)[3] > 200 || at(15, 8)[3] > 200, "the ring is solid at radius 8");
        assert!(at(16, 11)[3] < 60, "between the dot and the ring it is clear");
        // Cyan where solid.
        let ring = at(15, 8);
        assert!(ring[2] > ring[0], "bluish: {ring:?}");
    }

    #[test]
    fn show_and_restore_do_nothing_under_test() {
        show_working_cursor();
        restore();
        let _guard = WorkingCursor::show();
    }
}
