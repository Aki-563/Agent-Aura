//! The harness's own window registry (HARNESS_COMPLETION_PLAN.md §H3).
//!
//! Every top-level window the user could work with gets a **stable id** (`w1`, `w2`, ...) the
//! first time it is seen; the id stays with that window for the life of the process and is never
//! reused. The registry is refreshed every step and keeps, per window: title, program, pid,
//! visible rectangle, monitor, z-order, state, which windows cover it, and whether it is hung.
//!
//! It feeds two things:
//! * the **window-state block** sent to the model with every screenshot (`state_block`), so the
//!   model always knows how many windows exist, which one is active, and what covers what;
//! * **validation** of actions aimed at a window that no longer exists, is minimized or is covered
//!   (`check_target`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, IsHungAppWindow, IsIconic, IsZoomed};

use crate::desktop::target;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WindowInfo {
    pub id: String,
    #[serde(skip)]
    pub hwnd: isize,
    pub title: String,
    pub process: String,
    pub pid: u32,
    /// Visible rectangle as [x, y, w, h] in physical pixels.
    pub rect: [i32; 4],
    pub monitor: usize,
    /// 0 = front-most.
    pub z: usize,
    pub state: WindowState,
    pub is_active: bool,
    pub is_target: bool,
    /// Ids of windows in front of this one that overlap it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub is_covered_by: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not_responding: bool,
}

/// One refresh of the registry.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub windows: Vec<WindowInfo>,
    pub monitors: Vec<[i32; 4]>,
    pub changes: Vec<String>,
}

impl Snapshot {
    pub fn active(&self) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.is_active)
    }

    pub fn by_id(&self, id: &str) -> Option<&WindowInfo> {
        let id = normalize_id(id)?;
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn minimized_count(&self) -> usize {
        self.windows.iter().filter(|w| w.state == WindowState::Minimized).count()
    }
}

struct Registry {
    ids: BTreeMap<isize, u32>,
    next: u32,
    last: Vec<WindowInfo>,
    last_active: Option<String>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry { ids: BTreeMap::new(), next: 1, last: Vec::new(), last_active: None });

fn lock() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// "w5", "5", "#5", "window 5" -> "w5". None when it is not an id.
pub fn normalize_id(text: &str) -> Option<String> {
    let lower = text.trim().to_lowercase();
    let digits = lower.trim_start_matches("window").trim().trim_start_matches(['w', '#']).trim_start_matches('.').trim();
    let number: u32 = digits.parse().ok()?;
    (number > 0).then(|| format!("w{number}"))
}

fn monitors() -> Vec<(isize, [i32; 4])> {
    unsafe extern "system" fn collect(monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR, _: windows_sys::Win32::Graphics::Gdi::HDC, _: *mut RECT, list: isize) -> i32 {
        let list = &mut *(list as *mut Vec<(isize, [i32; 4])>);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let r = info.rcMonitor;
            list.push((monitor as isize, [r.left, r.top, r.right - r.left, r.bottom - r.top]));
        }
        1
    }
    let mut list: Vec<(isize, [i32; 4])> = Vec::new();
    unsafe { windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut list as *mut _ as isize) };
    list
}

/// Pure part of a refresh: z-order, covered-by and state from raw facts. Separate for tests.
pub fn compute_cover(windows: &mut [WindowInfo]) {
    let shown: Vec<(usize, [i32; 4])> = windows.iter().enumerate().filter(|(_, w)| w.state != WindowState::Minimized).map(|(i, w)| (i, w.rect)).collect();
    for i in 0..windows.len() {
        if windows[i].state == WindowState::Minimized {
            continue;
        }
        let me = windows[i].rect;
        let mut covers = Vec::new();
        for &(j, other) in &shown {
            if j >= i {
                break; // only windows in front (lower z) can cover
            }
            let overlap = me[0] < other[0] + other[2] && other[0] < me[0] + me[2] && me[1] < other[1] + other[3] && other[1] < me[1] + me[3];
            if overlap {
                covers.push(windows[j].id.clone());
            }
        }
        windows[i].is_covered_by = covers;
    }
}

/// Differences between two refreshes, phrased for the model.
pub fn diff(before: &[WindowInfo], after: &[WindowInfo], active_before: Option<&str>, active_after: Option<&str>) -> Vec<String> {
    let mut changes = Vec::new();
    for w in after {
        match before.iter().find(|b| b.id == w.id) {
            None => changes.push(format!("{} opened: '{}'", w.id, w.title)),
            Some(b) => {
                if b.state != w.state {
                    changes.push(format!("{} is now {}", w.id, format!("{:?}", w.state).to_lowercase()));
                } else if b.rect != w.rect && w.state == WindowState::Normal {
                    let moved = b.rect[0] != w.rect[0] || b.rect[1] != w.rect[1];
                    let resized = b.rect[2] != w.rect[2] || b.rect[3] != w.rect[3];
                    changes.push(format!("{} {}", w.id, match (moved, resized) { (true, true) => "moved and resized", (true, false) => "moved", _ => "resized" }));
                }
                if b.title != w.title {
                    changes.push(format!("{} title changed to '{}'", w.id, w.title));
                }
                if !b.not_responding && w.not_responding {
                    changes.push(format!("{} stopped responding", w.id));
                }
            }
        }
    }
    for b in before {
        if !after.iter().any(|w| w.id == b.id) {
            changes.push(format!("{} closed: '{}'", b.id, b.title));
        }
    }
    if active_before != active_after {
        if let Some(after) = active_after {
            changes.push(match active_before {
                Some(before) => format!("focus moved from {before} to {after}"),
                None => format!("focus is on {after}"),
            });
        }
    }
    changes
}

/// Re-reads every window from Windows, keeps ids stable, and records what changed.
pub fn refresh() -> Snapshot {
    let mut raw: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(target::collect_windows), &mut raw as *mut Vec<HWND> as isize) };
    let active = target::effective_foreground();
    let target_hwnd = target::target();
    let monitor_list = monitors();
    let mut registry = lock();
    let mut windows = Vec::new();
    for hwnd in raw {
        if target::is_own_window(hwnd) || !target::is_candidate_window_with(hwnd, true) {
            continue;
        }
        let key = hwnd as isize;
        let number = match registry.ids.get(&key) {
            Some(n) => *n,
            None => {
                let n = registry.next;
                registry.next += 1;
                registry.ids.insert(key, n);
                n
            }
        };
        let (l, t, r, b) = target::visible_bounds(hwnd).unwrap_or((0, 0, 0, 0));
        let monitor_handle = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) } as isize;
        let monitor_index = monitor_list.iter().position(|(h, _)| *h == monitor_handle).unwrap_or(0);
        let monitor_rect = monitor_list.get(monitor_index).map(|(_, r)| *r).unwrap_or([0, 0, 0, 0]);
        let state = unsafe {
            if IsIconic(hwnd) != 0 {
                WindowState::Minimized
            } else if IsZoomed(hwnd) != 0 {
                WindowState::Maximized
            } else if [l, t, r - l, b - t] == monitor_rect {
                WindowState::Fullscreen
            } else {
                WindowState::Normal
            }
        };
        windows.push(WindowInfo {
            id: format!("w{number}"),
            hwnd: key,
            title: target::title_of(hwnd),
            process: target::process_name_of(hwnd),
            pid: target::process_of(hwnd),
            rect: [l, t, r - l, b - t],
            monitor: monitor_index + 1,
            z: windows.len(),
            state,
            is_active: Some(hwnd) == active,
            is_target: Some(hwnd) == target_hwnd,
            is_covered_by: Vec::new(),
            not_responding: unsafe { IsHungAppWindow(hwnd) } != 0,
        });
    }
    compute_cover(&mut windows);
    let active_id = windows.iter().find(|w| w.is_active).map(|w| w.id.clone());
    let changes = if registry.last.is_empty() { Vec::new() } else { diff(&registry.last, &windows, registry.last_active.as_deref(), active_id.as_deref()) };
    registry.last = windows.clone();
    registry.last_active = active_id;
    // Forget handles that no longer exist so the map does not grow forever (ids are not reused).
    let alive: Vec<isize> = windows.iter().map(|w| w.hwnd).collect();
    registry.ids.retain(|hwnd, _| alive.contains(hwnd) || target::is_alive(*hwnd as HWND));
    Snapshot { windows, monitors: monitor_list.into_iter().map(|(_, r)| r).collect(), changes }
}

/// The last refresh without touching Windows (refreshes once if there is none yet).
pub fn last() -> Snapshot {
    let windows = lock().last.clone();
    if windows.is_empty() {
        return refresh();
    }
    Snapshot { windows, monitors: monitors().into_iter().map(|(_, r)| r).collect(), changes: Vec::new() }
}

/// The window behind a stable id ("w5", "5", "#5"), if it still exists.
pub fn resolve(reference: &str) -> Option<HWND> {
    let id = normalize_id(reference)?;
    let mut found = lock().last.iter().find(|w| w.id == id).map(|w| w.hwnd as HWND);
    if found.is_none() {
        found = refresh().windows.iter().find(|w| w.id == id).map(|w| w.hwnd as HWND);
    }
    found.filter(|hwnd| target::is_alive(*hwnd))
}

/// The stable id of a window handle (assigning one if new).
pub fn id_of(hwnd: HWND) -> Option<String> {
    let known = lock().ids.get(&(hwnd as isize)).copied();
    known.map(|n| format!("w{n}")).or_else(|| refresh().windows.iter().find(|w| w.hwnd == hwnd as isize).map(|w| w.id.clone()))
}

/// What the attached screenshot covers, so coordinates map back exactly.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotScope {
    /// "full_screen", "monitor N", "window wN" or "zoom".
    pub scope: String,
    pub monitor: usize,
    pub width: u32,
    pub height: u32,
    /// Image pixels per physical pixel.
    pub scale: f64,
    /// Physical position of the image's top-left corner.
    pub offset: [i32; 2],
}

/// The window-state block sent with every screenshot (HARNESS_COMPLETION_PLAN.md §H3).
pub fn state_block(snapshot: &Snapshot, screenshot: &ScreenshotScope) -> Value {
    let compact = |w: &WindowInfo| {
        let mut value = json!({
            "id": w.id, "title": truncate(&w.title, 70), "process": w.process, "rect": w.rect,
            "monitor": w.monitor, "z": w.z, "state": w.state,
        });
        if w.is_active {
            value["is_active"] = json!(true);
        }
        if w.is_target {
            value["is_target"] = json!(true);
        }
        if !w.is_covered_by.is_empty() {
            value["is_covered_by"] = json!(w.is_covered_by);
        }
        if w.not_responding {
            value["not_responding"] = json!(true);
        }
        value
    };
    let shown: Vec<&WindowInfo> = snapshot.windows.iter().take(20).collect();
    json!({
        "window_count": snapshot.windows.len(),
        "minimized_count": snapshot.minimized_count(),
        "active_window": snapshot.active().map(|w| json!({"id": w.id, "title": truncate(&w.title, 70), "process": w.process, "rect": w.rect, "state": w.state})),
        "windows": shown.into_iter().map(compact).collect::<Vec<_>>(),
        "screen": {"monitors": snapshot.monitors},
        "screenshot": screenshot,
        "changes": snapshot.changes,
    })
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Why an action aimed at `window` must not run, if it must not.
pub fn check_target(snapshot: &Snapshot, window: &str) -> Result<(), String> {
    let id = normalize_id(window).ok_or_else(|| format!("'{window}' is not a window id; use an id from the window list (for example w3)."))?;
    let Some(w) = snapshot.windows.iter().find(|w| w.id == id) else {
        return Err(format!("{id} no longer exists (it was closed). Look at the window list again."));
    };
    if w.state == WindowState::Minimized {
        return Err(format!("{id} '{}' is minimized; restore or focus it first.", truncate(&w.title, 50)));
    }
    if w.not_responding {
        return Err(format!("{id} '{}' is not responding; wait, or ask the user.", truncate(&w.title, 50)));
    }
    if !w.is_active {
        return Err(format!("{id} '{}' is not the active window; focus it first (focus_window {id}).", truncate(&w.title, 50)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, rect: [i32; 4], state: WindowState, z: usize) -> WindowInfo {
        WindowInfo {
            id: id.into(), hwnd: z as isize + 100, title: format!("{id} title"), process: "app.exe".into(), pid: 1, rect, monitor: 1, z,
            state, is_active: z == 0, is_target: z == 0, is_covered_by: Vec::new(), not_responding: false,
        }
    }

    #[test]
    fn ids_normalize_from_every_spelling() {
        for text in ["w5", "W5", "5", "#5", "window 5", " w 5"] {
            assert_eq!(normalize_id(text).as_deref(), Some("w5"), "{text}");
        }
        assert_eq!(normalize_id("notepad"), None);
        assert_eq!(normalize_id("0"), None);
    }

    #[test]
    fn only_windows_in_front_that_overlap_cover_a_window() {
        let mut windows = vec![
            window("w3", [120, 80, 900, 600], WindowState::Normal, 0),
            window("w9", [2000, 0, 300, 300], WindowState::Normal, 1),
            window("w4", [1500, 0, 400, 400], WindowState::Minimized, 2),
            window("w1", [0, 0, 960, 540], WindowState::Normal, 3),
        ];
        compute_cover(&mut windows);
        assert_eq!(windows[3].is_covered_by, vec!["w3".to_owned()]);
        assert!(windows[0].is_covered_by.is_empty(), "the front window is covered by nothing");
        assert!(windows[1].is_covered_by.is_empty(), "no overlap");
    }

    #[test]
    fn diff_reports_open_close_move_state_and_focus() {
        let before = vec![window("w1", [0, 0, 500, 500], WindowState::Normal, 0), window("w2", [10, 10, 300, 300], WindowState::Normal, 1)];
        let mut after = vec![window("w1", [40, 0, 500, 500], WindowState::Normal, 1), window("w5", [0, 0, 200, 200], WindowState::Normal, 0)];
        after[0].title = "renamed".into();
        let changes = diff(&before, &after, Some("w1"), Some("w5"));
        let text = changes.join(" | ");
        for wanted in ["w5 opened", "w2 closed", "w1 moved", "w1 title changed", "focus moved from w1 to w5"] {
            assert!(text.contains(wanted), "missing '{wanted}' in {text}");
        }
        let minimized = vec![window("w1", [0, 0, 500, 500], WindowState::Minimized, 0)];
        assert!(diff(&before[..1], &minimized, None, None).join(" ").contains("w1 is now minimized"));
    }

    #[test]
    fn the_block_has_counts_active_window_and_screenshot_scope() {
        let mut windows = vec![window("w3", [120, 80, 900, 600], WindowState::Normal, 0), window("w1", [0, 0, 960, 540], WindowState::Normal, 1), window("w2", [0, 0, 10, 10], WindowState::Minimized, 2)];
        compute_cover(&mut windows);
        let snapshot = Snapshot { windows, monitors: vec![[0, 0, 1920, 1080]], changes: vec!["w2 closed: 'x'".into()] };
        let scope = ScreenshotScope { scope: "full_screen".into(), monitor: 1, width: 1280, height: 720, scale: 1280.0 / 1920.0, offset: [0, 0] };
        let block = state_block(&snapshot, &scope);
        assert_eq!(block["window_count"], 3);
        assert_eq!(block["minimized_count"], 1);
        assert_eq!(block["active_window"]["id"], "w3");
        assert_eq!(block["windows"][1]["is_covered_by"][0], "w3");
        assert_eq!(block["screenshot"]["scope"], "full_screen");
        assert_eq!(block["screenshot"]["width"], 1280);
        assert_eq!(block["changes"][0], "w2 closed: 'x'");
    }

    #[test]
    fn actions_on_closed_minimized_or_background_windows_are_refused() {
        let snapshot = Snapshot {
            windows: vec![window("w3", [0, 0, 100, 100], WindowState::Normal, 0), window("w2", [0, 0, 100, 100], WindowState::Minimized, 1), window("w1", [0, 0, 100, 100], WindowState::Normal, 2)],
            monitors: vec![],
            changes: vec![],
        };
        assert!(check_target(&snapshot, "w3").is_ok());
        assert!(check_target(&snapshot, "w9").unwrap_err().contains("no longer exists"));
        assert!(check_target(&snapshot, "w2").unwrap_err().contains("minimized"));
        assert!(check_target(&snapshot, "w1").unwrap_err().contains("not the active window"));
    }
}
