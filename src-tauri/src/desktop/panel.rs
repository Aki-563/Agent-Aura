//! Aura's control window while a task runs: a small, non-focusable, always-on-top panel in a
//! screen corner. It never minimizes or hides (which would hand focus to a random window); it
//! only moves out of the way when the agent needs to click where it sits.

use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};

/// The mini player: a compact capsule that grows when hovered (the page asks for its height).
const PANEL_WIDTH_DIP: f64 = 360.0;
/// Collapsed height: the capsule plus a few transparent pixels around it (no shadow).
pub const MINI_HEIGHT_DIP: f64 = 68.0;
/// Tallest the player may grow (approval card, steer box).
pub const MAX_HEIGHT_DIP: f64 = 460.0;
const MARGIN_DIP: f64 = 12.0;

static HEIGHT_DIP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(MINI_HEIGHT_DIP as u32);

/// Where the user last dropped the player (physical top-left), kept between runs.
static USER_POSITION: std::sync::Mutex<Option<Option<(i32, i32)>>> = std::sync::Mutex::new(None);

fn position_file() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("AgentAura").join("mini_position.json"))
}

fn user_position() -> Option<(i32, i32)> {
    let mut cached = USER_POSITION.lock().unwrap_or_else(|e| e.into_inner());
    if cached.is_none() {
        let loaded = position_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<(i32, i32)>(&t).ok());
        *cached = Some(loaded);
    }
    cached.flatten()
}

/// Remembers where the player is now (called after the user dragged it).
pub fn remember_position(window: &WebviewWindow) {
    let Ok(position) = window.outer_position() else { return };
    let point = (position.x, position.y);
    *USER_POSITION.lock().unwrap_or_else(|e| e.into_inner()) = Some(Some(point));
    if let Some(path) = position_file() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_string(&point).unwrap_or_default());
    }
}

/// The remembered place, pulled fully onto the monitor (a monitor may have been unplugged).
pub fn clamp_to_monitor(point: (i32, i32), monitor_pos: (i32, i32), monitor_size: (u32, u32), panel: (u32, u32)) -> (i32, i32) {
    let max_x = monitor_pos.0 + monitor_size.0 as i32 - panel.0 as i32;
    let max_y = monitor_pos.1 + monitor_size.1 as i32 - panel.1 as i32;
    (point.0.clamp(monitor_pos.0, max_x.max(monitor_pos.0)), point.1.clamp(monitor_pos.1, max_y.max(monitor_pos.1)))
}

/// Brings a minimized window back with Windows itself. (The window library's own "is it
/// minimized" flag misses a minimize done from outside, by Win+D or "minimize all", and its
/// restore then does nothing.) Returns whether it was minimized.
pub fn restore_if_minimized(window: &WebviewWindow) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindow, SW_RESTORE};
    let Ok(hwnd) = window.hwnd() else { return false };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    unsafe {
        if IsIconic(hwnd) == 0 {
            return false;
        }
        ShowWindow(hwnd, SW_RESTORE);
    }
    true
}

/// Window geometry to restore after the run.
pub struct SavedLayout {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    was_maximized: bool,
    /// Closed to the tray when a scheduled run started: hidden again afterwards.
    was_hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

/// Preference order: the right-hand side of the screen first, then the left.
const CORNERS: [Corner; 4] = [Corner::TopRight, Corner::BottomRight, Corner::TopLeft, Corner::BottomLeft];

/// Top-left position of a `width` x `height` panel placed in `corner` of a monitor.
pub fn corner_position(
    corner: Corner,
    monitor_pos: (i32, i32),
    monitor_size: (u32, u32),
    panel: (u32, u32),
    margin: i32,
) -> (i32, i32) {
    let right = monitor_pos.0 + monitor_size.0 as i32 - panel.0 as i32 - margin;
    let bottom = monitor_pos.1 + monitor_size.1 as i32 - panel.1 as i32 - margin;
    let left = monitor_pos.0 + margin;
    let top = monitor_pos.1 + margin;
    match corner {
        Corner::BottomRight => (right, bottom),
        Corner::BottomLeft => (left, bottom),
        Corner::TopRight => (right, top),
        Corner::TopLeft => (left, top),
    }
}

fn rect_contains(position: (i32, i32), size: (u32, u32), point: (i32, i32)) -> bool {
    point.0 >= position.0
        && point.0 < position.0 + size.0 as i32
        && point.1 >= position.1
        && point.1 < position.1 + size.1 as i32
}

/// First corner whose panel rectangle contains none of the points (with a safety margin).
pub fn corner_avoiding(
    monitor_pos: (i32, i32),
    monitor_size: (u32, u32),
    panel: (u32, u32),
    margin: i32,
    points: &[(i32, i32)],
) -> Option<Corner> {
    CORNERS.into_iter().find(|corner| {
        let position = corner_position(*corner, monitor_pos, monitor_size, panel, margin);
        let padded_pos = (position.0 - 24, position.1 - 24);
        let padded_size = (panel.0 + 48, panel.1 + 48);
        !points.iter().any(|point| rect_contains(padded_pos, padded_size, *point))
    })
}

#[derive(Clone, Copy)]
struct MonitorGeometry {
    position: (i32, i32),
    size: (u32, u32),
    scale: f64,
}

fn monitor_of(window: &WebviewWindow) -> Option<MonitorGeometry> {
    let monitor = window.current_monitor().ok().flatten().or_else(|| window.primary_monitor().ok().flatten())?;
    Some(MonitorGeometry {
        position: (monitor.position().x, monitor.position().y),
        size: (monitor.size().width, monitor.size().height),
        scale: monitor.scale_factor(),
    })
}

/// The monitor that contains `point` (where the player was dropped), or None when that monitor is
/// gone. The remembered place is only used on its own monitor, so a changed layout or mixed
/// scaling never puts the player somewhere odd.
fn monitor_at(window: &WebviewWindow, point: (i32, i32)) -> Option<MonitorGeometry> {
    window.available_monitors().ok()?.into_iter().find_map(|m| {
        let (pos, size) = ((m.position().x, m.position().y), (m.size().width, m.size().height));
        rect_contains(pos, size, point).then(|| MonitorGeometry { position: pos, size, scale: m.scale_factor() })
    })
}

fn panel_size(scale: f64) -> (u32, u32) {
    let height = HEIGHT_DIP.load(std::sync::atomic::Ordering::SeqCst) as f64;
    ((PANEL_WIDTH_DIP * scale) as u32, (height * scale) as u32)
}

/// Resizes the player to `height_dip` keeping its top-left corner (or moving it up when it would
/// otherwise run off the bottom of the screen). Called by the page as the capsule expands.
pub fn set_height(window: &WebviewWindow, height_dip: f64) {
    let height = height_dip.clamp(MINI_HEIGHT_DIP, MAX_HEIGHT_DIP).round();
    HEIGHT_DIP.store(height as u32, std::sync::atomic::Ordering::SeqCst);
    let Some(monitor) = monitor_of(window) else { return };
    let size = panel_size(monitor.scale);
    let _ = window.set_size(PhysicalSize::new(size.0, size.1));
    if let Ok(position) = window.outer_position() {
        let bottom_limit = monitor.position.1 + monitor.size.1 as i32 - (MARGIN_DIP * monitor.scale) as i32;
        let overflow = position.y + size.1 as i32 - bottom_limit;
        if overflow > 0 {
            let _ = window.set_position(PhysicalPosition::new(position.x, position.y - overflow));
        }
    }
}

/// Shrinks the window into a non-focusable corner panel. Returns what to restore afterwards.
pub fn enter_run_mode(window: &WebviewWindow) -> Option<SavedLayout> {
    // A minimized window reports the tiny iconic size; restoring that afterwards left the app as a
    // small rectangle. Take the real size first.
    restore_if_minimized(window);
    let was_hidden = !window.is_visible().unwrap_or(true);
    let saved = SavedLayout {
        position: window.outer_position().ok()?,
        // `set_size` sets the inner size, so the inner size is what is kept: keeping the outer one
        // made the window grow by its frame after every run.
        size: window.inner_size().ok()?,
        was_maximized: window.is_maximized().unwrap_or(false),
        was_hidden,
    };
    // The remembered place is used on the monitor it was saved on; otherwise the top-right corner
    // of the window's own monitor.
    let remembered = user_position().and_then(|point| monitor_at(window, point).map(|monitor| (point, monitor)));
    let monitor = match &remembered {
        Some((_, monitor)) => *monitor,
        None => monitor_of(window)?,
    };
    let margin = (MARGIN_DIP * monitor.scale) as i32;
    if saved.was_maximized {
        let _ = window.unmaximize();
    }
    // A frameless capsule: no title bar, no border, no taskbar button, never takes the keyboard.
    HEIGHT_DIP.store(MINI_HEIGHT_DIP as u32, std::sync::atomic::Ordering::SeqCst);
    let panel = panel_size(monitor.scale);
    // The home window's minimum size would stop it from becoming a small capsule.
    let _ = window.set_min_size(None::<tauri::Size>);
    let _ = window.set_resizable(false);
    let _ = window.set_decorations(false);
    let _ = window.set_shadow(false);
    let _ = window.set_skip_taskbar(true);
    let (x, y) = match remembered {
        Some((point, _)) => clamp_to_monitor(point, monitor.position, monitor.size, panel),
        None => corner_position(Corner::TopRight, monitor.position, monitor.size, panel, margin),
    };
    // Moved first, then sized: on a monitor with another scale the size is then right.
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(panel.0, panel.1));
    let _ = window.set_always_on_top(true);
    let _ = window.set_focusable(false);
    if was_hidden {
        let _ = window.show();
    }
    Some(saved)
}

/// Restores the normal window after a run, exactly as it was when the run started (size,
/// place, maximized or not), un-minimized and in front of the app the agent worked in.
pub fn leave_run_mode(window: &WebviewWindow, saved: Option<SavedLayout>) {
    // Something during the run (Win+D, "minimize all") may have minimized the player: a minimized
    // window ignores size and place changes, so it comes back first.
    restore_if_minimized(window);
    let _ = window.set_focusable(true);
    let _ = window.set_always_on_top(false);
    let _ = window.set_decorations(true);
    let _ = window.set_shadow(true);
    let _ = window.set_resizable(true);
    let _ = window.set_skip_taskbar(false);
    let _ = window.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(640.0, 480.0))));
    let was_hidden = saved.as_ref().is_some_and(|s| s.was_hidden);
    let restored = saved.as_ref().map(|s| SavedLayout { position: s.position, size: s.size, was_maximized: s.was_maximized, was_hidden: s.was_hidden });
    if let Some(saved) = saved.filter(|s| s.size.width >= 400 && s.size.height >= 300) {
        let _ = window.set_size(saved.size);
        let _ = window.set_position(saved.position);
        if saved.was_maximized {
            let _ = window.maximize();
        }
    } else {
        let _ = window.set_size(tauri::LogicalSize::new(1040.0, 720.0));
        let _ = window.center();
    }
    if was_hidden {
        // A scheduled run while Aura sat in the tray: back to the tray, not onto the screen.
        let _ = window.hide();
        return;
    }
    bring_forward(window);
    // A minimize that lands while the window is being put back (Win+D at the last moment) can
    // leave it parked off screen: check once more and put it back again.
    if let Some(saved) = restored.filter(|s| s.size.width >= 400 && s.size.height >= 300) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let parked = window.outer_position().map(|p| p.x <= -20000 || p.y <= -20000).unwrap_or(false);
        if restore_if_minimized(window) || parked {
            let _ = window.set_size(saved.size);
            let _ = window.set_position(saved.position);
            if saved.was_maximized {
                let _ = window.maximize();
            }
            bring_forward(window);
        }
    }
}

/// Shows the window in front of every other one. Windows refuses a plain "set focus" from an app
/// that is not in front (the agent's app is), which used to leave Aura behind it after a run, as if
/// minimized; this uses the same forced hand-over the agent uses for its own target windows.
fn bring_forward(window: &WebviewWindow) {
    let _ = window.show();
    restore_if_minimized(window);
    let _ = window.set_focus();
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    for _ in 0..4 {
        if super::target::bring_to_front(hwnd) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
}

/// During a run the player is never left minimized (Win+D, "minimize all", a stray minimize).
/// A window hidden in the tray is not shown here: entering run mode does that, and remembers it.
pub fn keep_player_shown(window: &WebviewWindow) {
    restore_if_minimized(window);
}

/// Called every few seconds during a run: when a menu or pop-up of the app being worked on opens
/// under the player, the player moves to a corner clear of it instead of staying on top of it.
pub fn keep_clear_of_popups(window: &WebviewWindow) {
    let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else { return };
    let panel = (position.x, position.y, position.x + size.width as i32, position.y + size.height as i32);
    let popups = popup_rects_over(panel);
    if popups.is_empty() {
        return;
    }
    // Avoid the whole pop-up: its corners, edges' middles and centre.
    let points: Vec<(i32, i32)> = popups
        .iter()
        .flat_map(|&(l, t, r, b)| {
            let (cx, cy) = ((l + r) / 2, (t + b) / 2);
            [(l, t), (r, t), (l, b), (r, b), (cx, t), (cx, b), (l, cy), (r, cy), (cx, cy)]
        })
        .collect();
    move_away_from(window, &points);
}

/// Screen rectangles of visible menus and pop-up windows of other programs that overlap `panel`.
fn popup_rects_over(panel: (i32, i32, i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, GWL_STYLE, WS_POPUP};
    struct Search {
        panel: (i32, i32, i32, i32),
        found: Vec<(i32, i32, i32, i32)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() {
            return 1;
        }
        let mut class = [0u16; 32];
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let is_menu = String::from_utf16_lossy(&class[..n]) == "#32768";
        let is_popup = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_POPUP != 0;
        if !(is_menu || is_popup) {
            return 1;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 {
            return 1;
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        // Small enough to be a menu, list or tooltip-like pop-up (not a full-screen overlay).
        let sized = w > 20 && h > 12 && w < 1600 && h < 1200;
        let (pl, pt, pr, pb) = search.panel;
        let overlaps = r.left < pr && r.right > pl && r.top < pb && r.bottom > pt;
        if sized && overlaps && (is_menu || crate::input::takeover::belongs_to_foreground_app(pid)) {
            search.found.push((r.left, r.top, r.right, r.bottom));
        }
        1
    }
    let mut search = Search { panel, found: Vec::new() };
    unsafe { EnumWindows(Some(each), &mut search as *mut Search as LPARAM) };
    search.found
}

/// Moves the panel to a corner that does not cover any of `points` (screen coordinates).
/// Returns false when no corner is free.
pub fn move_away_from(window: &WebviewWindow, points: &[(i32, i32)]) -> bool {
    let Some(monitor) = monitor_of(window) else { return false };
    let panel = panel_size(monitor.scale);
    let margin = (MARGIN_DIP * monitor.scale) as i32;
    // The user's own place first, when it is clear of the points.
    if let Some(point) = user_position() {
        let (x, y) = clamp_to_monitor(point, monitor.position, monitor.size, panel);
        let padded = ((x - 24, y - 24), (panel.0 + 48, panel.1 + 48));
        if !points.iter().any(|p| rect_contains(padded.0, padded.1, *p)) {
            return window.set_position(PhysicalPosition::new(x, y)).is_ok();
        }
    }
    let Some(corner) = corner_avoiding(monitor.position, monitor.size, panel, margin, points) else {
        return false;
    };
    let (x, y) = corner_position(corner, monitor.position, monitor.size, panel, margin);
    window.set_position(PhysicalPosition::new(x, y)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_inside_the_monitor() {
        let pos = corner_position(Corner::BottomRight, (0, 0), (1920, 1080), (440, 620), 12);
        assert_eq!(pos, (1468, 448));
        let pos = corner_position(Corner::TopLeft, (1920, 0), (1920, 1080), (440, 620), 12);
        assert_eq!(pos, (1932, 12));
    }

    #[test]
    fn picks_a_free_corner_when_the_default_is_covered() {
        let monitor = ((0, 0), (1920, 1080));
        // A click near the top-right (the preferred side) forces another corner.
        let corner = corner_avoiding(monitor.0, monitor.1, (440, 620), 12, &[(1700, 200)]).unwrap();
        assert_ne!(corner, Corner::TopRight);
        // No points: the preferred corner is kept.
        assert_eq!(corner_avoiding(monitor.0, monitor.1, (440, 620), 12, &[]), Some(Corner::TopRight));
    }

    #[test]
    fn a_remembered_place_is_pulled_back_onto_the_monitor() {
        assert_eq!(clamp_to_monitor((1800, 900), (0, 0), (1920, 1080), (360, 68)), (1560, 900));
        assert_eq!(clamp_to_monitor((-500, -20), (0, 0), (1920, 1080), (360, 68)), (0, 0));
        assert_eq!(clamp_to_monitor((400, 300), (0, 0), (1920, 1080), (360, 68)), (400, 300));
    }

    #[test]
    fn reports_none_when_every_corner_is_blocked() {
        let monitor = ((0, 0), (900, 700));
        let points = [(100, 100), (800, 100), (100, 600), (800, 600)];
        assert_eq!(corner_avoiding(monitor.0, monitor.1, (440, 620), 12, &points), None);
    }
}
