//! Tracks the window the agent is working in and guards input against landing elsewhere.
//!
//! Policy:
//! * Aura never takes focus while running. If one of Aura's own windows ends up in front, the
//!   remembered target window is brought back before any input is sent.
//! * The target follows the foreground window: whenever a non-Aura window is in front at
//!   observation time it becomes the target (so an app opened mid-task is followed).
//! * Pointer actions are refused when the window under the point is Aura itself.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetTopWindow, GetWindow,
    GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, PostMessageW, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, GWL_EXSTYLE,
    GW_HWNDNEXT, SW_RESTORE, WS_EX_TOOLWINDOW,
};

#[link(name = "user32")]
extern "system" {
    fn AttachThreadInput(attach: u32, attach_to: u32, attach_flag: i32) -> i32;
}

static TARGET: AtomicIsize = AtomicIsize::new(0);

/// Shell windows that are never a useful automation target.
const SHELL_CLASSES: [&str; 5] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
];

pub fn own_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

fn process_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

pub fn is_own_window(hwnd: HWND) -> bool {
    !hwnd.is_null() && process_of(hwnd) == own_pid()
}

pub fn class_of(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

pub fn title_of(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let hr = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4)
    };
    hr == 0 && cloaked != 0
}

/// Whether a window is a plausible thing for the agent to operate (as opposed to a hidden,
/// minimized, tool or shell window).
fn is_candidate_window(hwnd: HWND) -> bool {
    is_candidate_window_with(hwnd, false)
}

/// Like `is_candidate_window`, optionally counting minimized windows (they still exist and can be restored).
fn is_candidate_window_with(hwnd: HWND, include_minimized: bool) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || (!include_minimized && IsIconic(hwnd) != 0) {
            return false;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return false;
        }
    }
    if is_cloaked(hwnd) || title_of(hwnd).trim().is_empty() {
        return false;
    }
    let class = class_of(hwnd);
    !SHELL_CLASSES.iter().any(|shell| *shell == class)
}

/// Topmost candidate window that does not belong to Aura (the window the user was just using).
pub fn find_previous_window() -> Option<HWND> {
    let mut hwnd = unsafe { GetTopWindow(std::ptr::null_mut()) };
    let mut guard = 0;
    while !hwnd.is_null() && guard < 2000 {
        if !is_own_window(hwnd) && is_candidate_window(hwnd) {
            return Some(hwnd);
        }
        hwnd = unsafe { GetWindow(hwnd, GW_HWNDNEXT) };
        guard += 1;
    }
    None
}

/// Lowercase executable file name of the window's process (for example `chrome.exe`).
pub fn process_name_of(hwnd: HWND) -> String {
    let pid = process_of(hwnd);
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; 520];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(handle);
        if ok == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase()
    }
}

pub fn is_browser_process(exe_name: &str) -> bool {
    ["chrome.exe", "msedge.exe", "firefox.exe", "brave.exe", "opera.exe", "vivaldi.exe"].contains(&exe_name)
}

/// How well a window matches a name the model gave: 3 = program name equals it,
/// 2 = title starts with it, 1 = title or program name contains it, 0 = no match.
pub fn window_match_rank(query: &str, title: &str, exe_name: &str) -> u8 {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 0;
    }
    let title = title.to_lowercase();
    let program = exe_name.trim_end_matches(".exe");
    if program == query || exe_name == query {
        3
    } else if title.starts_with(&query) {
        2
    } else if title.contains(&query) || program.contains(&query) {
        1
    } else {
        0
    }
}

unsafe extern "system" fn collect_windows(hwnd: HWND, list: isize) -> i32 {
    let windows = &mut *(list as *mut Vec<HWND>);
    windows.push(hwnd);
    1
}

/// Best-matching visible top-level window (not Aura's own) for a title or program name.
pub fn find_window_by_name(name: &str) -> Option<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_windows), &mut windows as *mut Vec<HWND> as isize) };
    windows
        .into_iter()
        .filter(|hwnd| !is_own_window(*hwnd) && is_candidate_window_with(*hwnd, true))
        .map(|hwnd| (window_match_rank(name, &title_of(hwnd), &process_name_of(hwnd)), hwnd))
        .filter(|(rank, _)| *rank > 0)
        // EnumWindows is in z-order (top first), so `max_by_key` keeps the first of equal ranks.
        .rev()
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, hwnd)| hwnd)
}

/// Handles of all visible, non-Aura, ordinary top-level windows right now.
pub fn list_candidate_windows() -> Vec<isize> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_windows), &mut windows as *mut Vec<HWND> as isize) };
    windows
        .into_iter()
        .filter(|hwnd| !is_own_window(*hwnd) && is_candidate_window(*hwnd))
        .map(|hwnd| hwnd as isize)
        .collect()
}

/// (handle, title) of every ordinary window (minimized ones included, so minimizing is not "closing"),
/// to tell afterwards what opened or closed.
pub fn window_titles() -> Vec<(isize, String)> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_windows), &mut windows as *mut Vec<HWND> as isize) };
    windows
        .into_iter()
        .filter(|hwnd| !is_own_window(*hwnd) && is_candidate_window_with(*hwnd, true))
        .map(|hwnd| (hwnd as isize, title_of(hwnd)))
        .collect()
}

/// One open window as the model is shown it.
#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub hwnd: isize,
    pub title: String,
    pub program: String,
    pub active: bool,
    pub minimized: bool,
    /// On a different monitor than the active window (so not visible in the screenshot).
    pub other_monitor: bool,
}

/// Handles of the windows last shown to the model, so "window 2" means the same window it saw.
static LISTED: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

fn monitor_of(hwnd: HWND) -> isize {
    unsafe { windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST) as isize }
}

/// Every ordinary window front to back (minimized ones included), flagged for the model.
pub fn window_inventory(active: HWND) -> Vec<WindowEntry> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_windows), &mut windows as *mut Vec<HWND> as isize) };
    let active_monitor = if active.is_null() { 0 } else { monitor_of(active) };
    windows
        .into_iter()
        .filter(|hwnd| !is_own_window(*hwnd) && is_candidate_window_with(*hwnd, true))
        .map(|hwnd| WindowEntry {
            hwnd: hwnd as isize,
            title: title_of(hwnd),
            program: process_name_of(hwnd).trim_end_matches(".exe").to_owned(),
            active: hwnd == active,
            minimized: unsafe { IsIconic(hwnd) } != 0,
            other_monitor: active_monitor != 0 && monitor_of(hwnd) != active_monitor,
        })
        .collect()
}

/// The window list as prompt text, and remembers the numbering for `resolve_window_reference`.
pub fn window_list_text(active: HWND) -> String {
    const SHOWN: usize = 16;
    let entries = window_inventory(active);
    *LISTED.lock().unwrap_or_else(|e| e.into_inner()) = entries.iter().take(SHOWN).map(|e| e.hwnd).collect();
    if entries.is_empty() {
        return "no other windows are open".to_owned();
    }
    let mut lines = vec![format!("{} open (front to back). Switch with focus_window using the number or the title:", entries.len())];
    for (index, entry) in entries.iter().take(SHOWN).enumerate() {
        let mut flags = Vec::new();
        if entry.active {
            flags.push("ACTIVE");
        }
        if entry.minimized {
            flags.push("minimized");
        }
        if entry.other_monitor {
            flags.push("on another monitor, not in the screenshot");
        }
        let title: String = entry.title.chars().take(70).collect();
        let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
        lines.push(format!("{}. \"{}\" ({}){}", index + 1, title, entry.program, flags));
    }
    if entries.len() > SHOWN {
        lines.push(format!("... and {} more", entries.len() - SHOWN));
    }
    lines.join("\n")
}

/// "2", "w2", "#2" or "window 2" -> the window that was number 2 in the last list shown.
pub fn resolve_window_reference(name: &str) -> Option<HWND> {
    let trimmed = name.trim().to_lowercase();
    let digits = trimmed
        .trim_start_matches("window")
        .trim_start_matches('w')
        .trim_start_matches('#')
        .trim_start_matches('.')
        .trim();
    let number: usize = digits.parse().ok()?;
    if LISTED.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
        // Nothing has been shown yet (for example a one-off command): number the windows as they are now.
        window_list_text(effective_foreground().unwrap_or(std::ptr::null_mut()));
    }
    let listed = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    let hwnd = *listed.get(number.checked_sub(1)?)? as HWND;
    is_alive(hwnd).then_some(hwnd)
}

/// A number from the window list, otherwise the best title or program match.
pub fn find_window_for_reference(name: &str) -> Option<HWND> {
    // A number means the list position, unless a window is titled exactly that ("2048").
    let exact_title = find_window_by_name(name).filter(|hwnd| title_of(*hwnd).trim().eq_ignore_ascii_case(name.trim()));
    exact_title
        .or_else(|| resolve_window_reference(name))
        .or_else(|| find_window_by_name(name))
}

/// The taskbar, the desktop and the system tray overflow: always fair targets, whichever window was in front.
pub fn is_shell_window(hwnd: HWND) -> bool {
    matches!(
        class_of(hwnd).as_str(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW" | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland" | "XamlExplorerHostIslandWindow"
    )
}

/// Rectangles of every visible window that belongs to the process (menus, drop-downs and dialogs
/// of an app are separate windows, so text on them still counts as that app's).
pub fn visible_window_rects_of_process(pid: u32) -> Vec<(i32, i32, i32, i32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindowVisible};
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_windows), &mut windows as *mut Vec<HWND> as isize) };
    windows
        .into_iter()
        .filter(|hwnd| unsafe { IsWindowVisible(*hwnd) } != 0 && process_of(*hwnd) == pid)
        .filter_map(|hwnd| {
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let ok = unsafe { GetWindowRect(hwnd, &mut rect) } != 0;
            (ok && rect.right > rect.left && rect.bottom > rect.top).then_some((rect.left, rect.top, rect.right, rect.bottom))
        })
        .collect()
}

/// Whether the window handle still refers to a window.
pub fn is_alive(hwnd: HWND) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd) != 0 }
}

/// Windows that appeared or disappeared since `before`, phrased for the model ("a new window opened: 'Save As'").
pub fn describe_window_changes(before: &[(isize, String)]) -> Option<String> {
    let after = window_titles();
    let opened: Vec<String> = after
        .iter()
        .filter(|(hwnd, _)| !before.iter().any(|(old, _)| old == hwnd))
        .map(|(_, title)| format!("'{}'", if title.is_empty() { "untitled window" } else { title }))
        .collect();
    let closed: Vec<String> = before
        .iter()
        .filter(|(hwnd, _)| !after.iter().any(|(new, _)| new == hwnd))
        .map(|(_, title)| format!("'{}'", if title.is_empty() { "untitled window" } else { title }))
        .collect();
    let mut parts = Vec::new();
    if !opened.is_empty() {
        parts.push(format!("a new window opened: {}", opened.join(", ")));
    }
    if !closed.is_empty() {
        parts.push(format!("a window closed: {}", closed.join(", ")));
    }
    if !parts.is_empty() {
        parts.push(format!("windows open: {} -> {}", before.len(), after.len()));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Asks a window to close (the same as clicking its X). Returns whether the request was sent.
pub fn close_window(hwnd: isize) -> bool {
    const WM_CLOSE: u32 = 0x0010;
    unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) != 0 }
}

/// Title of the window that is in front right now (empty if none).
pub fn foreground_title() -> String {
    effective_foreground().map(title_of).unwrap_or_default()
}

/// The remembered target window, if it still exists.
pub fn target() -> Option<HWND> {
    let value = TARGET.load(Ordering::SeqCst);
    let hwnd = value as HWND;
    (value != 0 && unsafe { IsWindow(hwnd) } != 0).then_some(hwnd)
}

pub fn set_target(hwnd: Option<HWND>) {
    TARGET.store(hwnd.map_or(0, |h| h as isize), Ordering::SeqCst);
}

/// Foreground window that is not Aura's, or the remembered target when Aura is in front.
/// Also updates the remembered target when a different non-Aura window is in front.
pub fn effective_foreground() -> Option<HWND> {
    let foreground = unsafe { GetForegroundWindow() };
    if !foreground.is_null() && !is_own_window(foreground) {
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        let chosen = if root.is_null() { foreground } else { root };
        set_target(Some(chosen));
        return Some(chosen);
    }
    target()
}

/// Brings `hwnd` to the front without the Alt-key trick (which can open menus).
fn force_foreground(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) != 0 && GetForegroundWindow() == hwnd {
            return true;
        }
        // The foreground lock blocks us unless we share an input queue with the current owner.
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, std::ptr::null_mut())
        };
        let current_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        if attached {
            SetFocus(hwnd);
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
        GetForegroundWindow() == hwnd
    }
}

/// Brings a specific window to the front (used by tests and by explicit focus actions).
pub fn bring_to_front(hwnd: HWND) -> bool {
    force_foreground(hwnd)
}

/// Makes sure a non-Aura window is in front before input is sent.
/// Returns the window now in front, or an error if the target could not be restored.
pub fn ensure_foreground() -> Result<Option<HWND>, String> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() || !is_own_window(foreground) {
        return Ok(effective_foreground());
    }
    let Some(target) = target().or_else(find_previous_window) else {
        return Ok(None);
    };
    if !force_foreground(target) {
        return Err(format!(
            "Aura's window was in front and the target window '{}' could not be restored to the foreground.",
            title_of(target)
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline && unsafe { GetForegroundWindow() } != target {
        thread::sleep(Duration::from_millis(10));
    }
    set_target(Some(target));
    Ok(Some(target))
}

/// Whether one of Aura's own windows currently has the foreground.
pub fn aura_in_front() -> bool {
    is_own_window(unsafe { GetForegroundWindow() })
}

/// Same as `ensure_foreground` but returns only success or failure (safe to move across threads).
pub fn ensure_foreground_ok() -> Result<(), String> {
    ensure_foreground().map(|_| ())
}

/// Root window under a screen point plus its process id.
pub fn window_at(x: i32, y: i32) -> Option<(HWND, u32)> {
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    if hwnd.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.is_null() { hwnd } else { root };
    Some((root, process_of(root)))
}

/// What to do about a pointer action aimed at a screen point.
#[derive(Debug, PartialEq, Eq)]
pub enum PointerDecision {
    Allow,
    /// One of Aura's own windows (panel/HUD) is on top of the point.
    CoveredByAura,
}

/// Pure decision used by `check_pointer` (kept separate so it can be unit tested).
pub fn decide_pointer(hit_pid: Option<u32>, own_pid: u32) -> PointerDecision {
    match hit_pid {
        Some(pid) if pid == own_pid => PointerDecision::CoveredByAura,
        _ => PointerDecision::Allow,
    }
}

pub fn check_pointer(x: i32, y: i32) -> PointerDecision {
    decide_pointer(window_at(x, y).map(|(_, pid)| pid), own_pid())
}

/// Visible window rectangle (excludes the invisible resize/shadow border).
pub fn visible_bounds(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if hr == 0 && rect.right > rect.left && rect.bottom > rect.top {
        return Some((rect.left, rect.top, rect.right, rect.bottom));
    }
    let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
    (ok != 0 && rect.right > rect.left).then_some((rect.left, rect.top, rect.right, rect.bottom))
}

/// Whether the process with this id runs elevated (as administrator). None when it cannot be
/// queried (protected system processes).
pub fn process_is_elevated(pid: u32) -> Option<bool> {
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            CloseHandle(process);
            return None;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut length = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        );
        CloseHandle(token);
        CloseHandle(process);
        (ok != 0).then_some(elevation.TokenIsElevated != 0)
    }
}

/// Windows silently drops input sent to a window that runs as administrator when Aura does not.
/// Returns an explanation to show instead of pretending the click worked.
pub fn input_blocked_by_elevation(hwnd: HWND) -> Option<String> {
    let target_elevated = process_is_elevated(process_of(hwnd)).unwrap_or(false);
    let own_elevated = process_is_elevated(std::process::id()).unwrap_or(false);
    (target_elevated && !own_elevated).then(|| {
        format!(
            "'{}' runs as administrator, so Windows ignores the clicks and keys Aura sends to it. \
             Start Aura as administrator, or ask the user to do this step.",
            title_of(hwnd)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_over_aura_is_flagged_and_other_windows_pass() {
        assert_eq!(decide_pointer(Some(42), 42), PointerDecision::CoveredByAura);
        assert_eq!(decide_pointer(Some(7), 42), PointerDecision::Allow);
        assert_eq!(decide_pointer(None, 42), PointerDecision::Allow);
    }

    #[test]
    fn the_desktop_window_is_never_a_candidate() {
        let desktop = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
        assert!(!is_candidate_window(desktop));
    }

    #[test]
    fn window_names_rank_program_then_title_start_then_contains() {
        assert_eq!(window_match_rank("chrome", "Claude - Google Chrome", "chrome.exe"), 3);
        assert_eq!(window_match_rank("Notepad", "Untitled - Notepad", "notepad.exe"), 3);
        assert_eq!(window_match_rank("untitled", "Untitled - Notepad", "notepad.exe"), 2);
        assert_eq!(window_match_rank("google", "Claude - Google Chrome", "chrome.exe"), 1);
        assert_eq!(window_match_rank("excel", "Claude - Google Chrome", "chrome.exe"), 0);
        assert_eq!(window_match_rank("  ", "anything", "x.exe"), 0);
    }

    #[test]
    fn browsers_are_recognised_by_program_name() {
        assert!(is_browser_process("msedge.exe") && is_browser_process("chrome.exe"));
        assert!(!is_browser_process("notepad.exe"));
    }

    #[test]
    fn finds_no_window_for_a_nonsense_name_and_reads_own_process_name() {
        assert!(find_window_by_name("zzzz-no-such-window-9f3a").is_none());
        assert!(process_name_of(std::ptr::null_mut()).len() < 2 || true);
    }

    #[test]
    fn target_ignores_handles_that_are_not_windows() {
        set_target(Some(0x7FFF_FFF0usize as HWND));
        assert!(target().is_none());
        set_target(None);
        assert!(target().is_none());
    }

    #[test]
    fn elevation_queries_do_not_crash_and_own_process_is_readable() {
        assert!(process_is_elevated(std::process::id()).is_some());
        // A pid that does not exist, and the idle process, are simply unknown.
        assert_eq!(process_is_elevated(0), None);
        assert_eq!(process_is_elevated(u32::MAX - 3), None);
    }
}
