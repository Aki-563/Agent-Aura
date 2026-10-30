//! Aura's control window while a task runs: a small, non-focusable, always-on-top panel in a
//! screen corner. It never minimizes or hides (which would hand focus to a random window); it
//! only moves out of the way when the agent needs to click where it sits.

use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};

const PANEL_WIDTH_DIP: f64 = 400.0;
const PANEL_HEIGHT_DIP: f64 = 600.0;
const MARGIN_DIP: f64 = 12.0;

/// Window geometry to restore after the run.
pub struct SavedLayout {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    was_maximized: bool,
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

fn panel_size(scale: f64) -> (u32, u32) {
    ((PANEL_WIDTH_DIP * scale) as u32, (PANEL_HEIGHT_DIP * scale) as u32)
}

/// Shrinks the window into a non-focusable corner panel. Returns what to restore afterwards.
pub fn enter_run_mode(window: &WebviewWindow) -> Option<SavedLayout> {
    let saved = SavedLayout {
        position: window.outer_position().ok()?,
        size: window.outer_size().ok()?,
        was_maximized: window.is_maximized().unwrap_or(false),
    };
    let monitor = monitor_of(window)?;
    let panel = panel_size(monitor.scale);
    let margin = (MARGIN_DIP * monitor.scale) as i32;
    if saved.was_maximized {
        let _ = window.unmaximize();
    }
    let _ = window.set_size(PhysicalSize::new(panel.0, panel.1));
    let (x, y) = corner_position(Corner::TopRight, monitor.position, monitor.size, panel, margin);
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_always_on_top(true);
    let _ = window.set_focusable(false);
    Some(saved)
}

/// Restores the normal window after a run and gives it focus again.
pub fn leave_run_mode(window: &WebviewWindow, saved: Option<SavedLayout>) {
    let _ = window.set_focusable(true);
    let _ = window.set_always_on_top(false);
    if let Some(saved) = saved {
        let _ = window.set_size(saved.size);
        let _ = window.set_position(saved.position);
        if saved.was_maximized {
            let _ = window.maximize();
        }
    }
    let _ = window.set_focus();
}

/// Moves the panel to a corner that does not cover any of `points` (screen coordinates).
/// Returns false when no corner is free.
pub fn move_away_from(window: &WebviewWindow, points: &[(i32, i32)]) -> bool {
    let Some(monitor) = monitor_of(window) else { return false };
    let panel = panel_size(monitor.scale);
    let margin = (MARGIN_DIP * monitor.scale) as i32;
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
    fn reports_none_when_every_corner_is_blocked() {
        let monitor = ((0, 0), (900, 700));
        let points = [(100, 100), (800, 100), (100, 600), (800, 600)];
        assert_eq!(corner_avoiding(monitor.0, monitor.1, (440, 620), 12, &points), None);
    }
}
