//! Window placement: tiling layouts (2, 3, 4, 6 windows), snapping, and exact resizing with
//! read-back. Pure layout math lives in `tiles` and `outer_for_visible` so it can be unit-tested;
//! `place` talks to Windows.
//!
//! Windows 10/11 windows have an invisible resize border (about 7 px left, right and bottom) that
//! `GetWindowRect` includes but the user does not see. Layouts are computed for the *visible*
//! rectangles (DWM extended frame bounds), and the rectangle passed to `SetWindowPos` is widened by
//! the measured invisible border so the visible edges land exactly where the layout says.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsZoomed, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE};

/// A rectangle as (left, top, right, bottom), physical pixels.
pub type Rect = (i32, i32, i32, i32);

/// Splits `area` into `count` tiles for the named layout, with `gap` pixels between tiles and
/// around the edge. Layouts: "columns", "rows", "grid" (default). Grids: 2 = side by side,
/// 3 = one large left + two stacked right, 4 = 2x2, 5-6 = 3x2, more = near-square grid.
pub fn tiles(area: Rect, count: usize, layout: &str, gap: i32) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let (left, top, right, bottom) = (area.0 + gap, area.1 + gap, area.2 - gap, area.3 - gap);
    let split = |start: i32, end: i32, parts: usize| -> Vec<(i32, i32)> {
        let total = end - start - gap * (parts as i32 - 1);
        (0..parts)
            .map(|i| {
                let a = start + (total * i as i32) / parts as i32 + gap * i as i32;
                let b = start + (total * (i as i32 + 1)) / parts as i32 + gap * i as i32;
                (a, b)
            })
            .collect()
    };
    match layout.trim().to_lowercase().as_str() {
        "columns" | "side_by_side" | "vertical" => split(left, right, count).into_iter().map(|(a, b)| (a, top, b, bottom)).collect(),
        "rows" | "stacked" | "horizontal" => split(top, bottom, count).into_iter().map(|(a, b)| (left, a, right, b)).collect(),
        _ => match count {
            1 => vec![(left, top, right, bottom)],
            2 => split(left, right, 2).into_iter().map(|(a, b)| (a, top, b, bottom)).collect(),
            3 => {
                let cols = split(left, right, 2);
                let rows = split(top, bottom, 2);
                vec![(cols[0].0, top, cols[0].1, bottom), (cols[1].0, rows[0].0, cols[1].1, rows[0].1), (cols[1].0, rows[1].0, cols[1].1, rows[1].1)]
            }
            _ => {
                let columns = match count {
                    4 => 2,
                    5 | 6 => 3,
                    n => (n as f64).sqrt().ceil() as usize,
                };
                let rows_needed = count.div_ceil(columns);
                let cols = split(left, right, columns);
                let rows = split(top, bottom, rows_needed);
                (0..count).map(|i| (cols[i % columns].0, rows[i / columns].0, cols[i % columns].1, rows[i / columns].1)).collect()
            }
        },
    }
}

/// Half or quarter of the work area for a snap side.
pub fn snap_rect(area: Rect, side: &str) -> Option<Rect> {
    let (l, t, r, b) = area;
    let (mx, my) = ((l + r) / 2, (t + b) / 2);
    Some(match side.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "left" => (l, t, mx, b),
        "right" => (mx, t, r, b),
        "top" => (l, t, r, my),
        "bottom" => (l, my, r, b),
        "top_left" => (l, t, mx, my),
        "top_right" => (mx, t, r, my),
        "bottom_left" => (l, my, mx, b),
        "bottom_right" => (mx, my, r, b),
        "full" | "maximize" => (l, t, r, b),
        _ => return None,
    })
}

/// The outer rectangle (what `SetWindowPos` takes) that makes the visible frame equal `visible`,
/// given the window's current outer and visible rectangles (their difference is the invisible border).
pub fn outer_for_visible(visible: Rect, current_outer: Rect, current_visible: Rect) -> Rect {
    let (bl, bt) = (current_visible.0 - current_outer.0, current_visible.1 - current_outer.1);
    let (br, bb) = (current_outer.2 - current_visible.2, current_outer.3 - current_visible.3);
    (visible.0 - bl, visible.1 - bt, visible.2 + br, visible.3 + bb)
}

/// The work area (screen minus taskbar) of the monitor a window is on.
pub fn work_area_of(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(monitor, &mut info) != 0).then_some((info.rcWork.left, info.rcWork.top, info.rcWork.right, info.rcWork.bottom))
    }
}

fn outer_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some((rect.left, rect.top, rect.right, rect.bottom))
}

/// What happened when a window was placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub wanted: Rect,
    pub got: Rect,
}

impl Placement {
    /// Visible edges within `tolerance` px of the wanted ones.
    pub fn exact(&self, tolerance: i32) -> bool {
        (self.wanted.0 - self.got.0).abs() <= tolerance
            && (self.wanted.1 - self.got.1).abs() <= tolerance
            && (self.wanted.2 - self.got.2).abs() <= tolerance
            && (self.wanted.3 - self.got.3).abs() <= tolerance
    }

    /// The app kept a larger size than asked (minimum size).
    pub fn refused_size(&self) -> bool {
        (self.got.2 - self.got.0) > (self.wanted.2 - self.wanted.0) + 2 || (self.got.3 - self.got.1) > (self.wanted.3 - self.wanted.1) + 2
    }
}

/// Moves and sizes a window so its visible frame is `visible`, then reads the frame back.
/// Restores a minimized or maximized window first (a maximized window ignores `SetWindowPos`).
pub fn place(hwnd: HWND, visible: Rect) -> Result<Placement, String> {
    unsafe {
        if IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    let mut got = (0, 0, 0, 0);
    // Twice: the invisible border can change after the first resize (DPI or style changes).
    for _ in 0..2 {
        let outer = outer_rect(hwnd).ok_or("the window is gone")?;
        let current_visible = crate::desktop::target::visible_bounds(hwnd).unwrap_or(outer);
        let (l, t, r, b) = outer_for_visible(visible, outer, current_visible);
        let ok = unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), l, t, r - l, b - t, SWP_NOZORDER | SWP_NOACTIVATE) };
        if ok == 0 {
            return Err("Windows refused to move the window (it may belong to an administrator program)".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        got = crate::desktop::target::visible_bounds(hwnd).ok_or("the window is gone")?;
        if (Placement { wanted: visible, got }).exact(1) {
            break;
        }
    }
    Ok(Placement { wanted: visible, got })
}

/// Places every window in its tile. When an app refuses a tile because of its minimum size, the
/// layout is not forced: the result says which window kept which size, so the caller can report it.
pub fn apply(windows: &[HWND], layout: &str, gap: i32) -> Result<Vec<Placement>, String> {
    let first = *windows.first().ok_or("no windows to arrange")?;
    let area = work_area_of(first).ok_or("could not read the screen's work area")?;
    let rects = tiles(area, windows.len(), layout, gap);
    windows.iter().zip(rects).map(|(hwnd, rect)| place(*hwnd, rect)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: Rect) -> i64 {
        (r.2 - r.0) as i64 * (r.3 - r.1) as i64
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
    }

    #[test]
    fn tiles_cover_the_work_area_without_overlap_for_2_3_4_6() {
        let work = (0, 0, 1920, 1032);
        for count in [2, 3, 4, 6] {
            for layout in ["grid", "columns", "rows"] {
                let rects = tiles(work, count, layout, 0);
                assert_eq!(rects.len(), count);
                for (i, a) in rects.iter().enumerate() {
                    assert!(a.0 >= 0 && a.1 >= 0 && a.2 <= 1920 && a.3 <= 1032, "{layout} {count}: inside");
                    for b in &rects[i + 1..] {
                        assert!(!overlaps(*a, *b), "{layout} {count}: {a:?} overlaps {b:?}");
                    }
                }
                let covered: i64 = rects.iter().map(|r| area(*r)).sum();
                assert_eq!(covered, area(work), "{layout} {count}: no pixel left over");
            }
        }
    }

    #[test]
    fn gaps_separate_tiles_and_edges() {
        let rects = tiles((0, 0, 1000, 600), 4, "grid", 8);
        assert_eq!(rects[0].0, 8);
        assert_eq!(rects[1].0 - rects[0].2, 8);
        assert_eq!(rects[2].1 - rects[0].3, 8);
        assert_eq!(rects[3].2, 992);
    }

    #[test]
    fn the_invisible_border_is_added_back_around_the_visible_frame() {
        // Win11: outer rect is 7 px wider left/right/bottom than the visible frame.
        let outer = (93, 100, 907, 607);
        let visible = (100, 100, 900, 600);
        assert_eq!(outer_for_visible((0, 0, 960, 1032), outer, visible), (-7, 0, 967, 1039));
    }

    #[test]
    fn snap_sides_and_placement_checks() {
        let work = (0, 0, 1920, 1032);
        assert_eq!(snap_rect(work, "left"), Some((0, 0, 960, 1032)));
        assert_eq!(snap_rect(work, "bottom-right"), Some((960, 516, 1920, 1032)));
        assert_eq!(snap_rect(work, "middle"), None);
        let refused = Placement { wanted: (0, 0, 300, 200), got: (0, 0, 500, 200) };
        assert!(refused.refused_size() && !refused.exact(1));
        assert!(Placement { wanted: (0, 0, 300, 200), got: (1, 0, 300, 201) }.exact(1));
    }
}
