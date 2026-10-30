//! Things that suddenly sit between the agent and its target: a notification toast, a tooltip or
//! hover card, an always-on-top widget (a desktop pet, a floating timer), a window that opened by
//! itself. The harness finds what covers the point it is about to click, decides what kind of thing
//! it is, and does only harmless things about it:
//!
//! * toast or tooltip: move the pointer away (hover cards close) and wait for it to disappear;
//! * always-on-top overlay: raise the target window above it for the duration of the action
//!   (nothing is closed or moved; [`restore_raised`] undoes it);
//! * a real window or dialog from another program: not touched; the model is told what it is.
//!
//! Nothing here clicks a button on an unknown window.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, GetWindowRect, SetCursorPos, SetWindowPos, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

use super::target::{class_of, is_alive, is_own_window, is_shell_window, process_name_of, process_of, title_of, window_at};

const WS_EX_TOPMOST: isize = 0x0000_0008;
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const WS_EX_NOACTIVATE: isize = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intruder {
    pub hwnd: isize,
    pub title: String,
    pub process: String,
    pub class: String,
    pub size: (i32, i32),
    pub topmost: bool,
    pub no_activate: bool,
    pub tool_window: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A notification toast (Windows or an app's own corner pop-up).
    Toast,
    /// A tooltip or hover card.
    Tooltip,
    /// An always-on-top widget that is not a normal window.
    Overlay,
    /// A normal window or dialog from another program.
    AppWindow,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Toast => "a notification",
            Kind::Tooltip => "a tooltip or hover card",
            Kind::Overlay => "an always-on-top widget",
            Kind::AppWindow => "another program's window",
        }
    }
}

pub fn describe(hwnd: HWND) -> Intruder {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let mut rect = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    Intruder {
        hwnd: hwnd as isize,
        title: title_of(hwnd),
        process: process_name_of(hwnd).trim_end_matches(".exe").to_owned(),
        class: class_of(hwnd),
        size: (rect.right - rect.left, rect.bottom - rect.top),
        topmost: style & WS_EX_TOPMOST != 0,
        no_activate: style & WS_EX_NOACTIVATE != 0,
        tool_window: style & WS_EX_TOOLWINDOW != 0,
    }
}

/// Decides what kind of thing a covering window is. Pure, so it is tested without a desktop.
pub fn classify(i: &Intruder) -> Kind {
    let class = i.class.to_lowercase();
    let process = i.process.to_lowercase();
    if class.contains("tooltips_class") || class == "tooltip" || class.contains("popuphost") {
        return Kind::Tooltip;
    }
    // Windows notifications live in the shell experience host; many apps draw their own toast as a
    // small topmost window that does not take focus.
    let shell_toast = process.contains("shellexperiencehost") || process.contains("shellhost") || class == "windows.ui.core.corewindow" && i.size.0 <= 520 && i.size.1 <= 260;
    if shell_toast {
        return Kind::Toast;
    }
    let small = i.size.0 <= 520 && i.size.1 <= 260;
    if i.topmost && (i.no_activate || i.tool_window) && small && !i.title.trim().is_empty() {
        return Kind::Toast;
    }
    // A small window with no title that cannot take focus is a hover card or a drop-down hint.
    if i.title.trim().is_empty() && i.no_activate && !i.topmost {
        return Kind::Tooltip;
    }
    if i.topmost && (i.no_activate || i.tool_window || i.title.trim().is_empty()) {
        return Kind::Overlay;
    }
    Kind::AppWindow
}

/// What happened while trying to uncover a point.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub cleared: bool,
    /// Plain-English steps for the model and the trace ("waited 2.1 s for a notification to go").
    pub steps: Vec<String>,
}

static RAISED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// What was done since the last call, for the trace and the next prompt.
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn take_notes() -> Vec<String> {
    std::mem::take(&mut *NOTES.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn note_external(text: String) {
    note(text);
}

fn note(text: String) {
    NOTES.lock().unwrap_or_else(|e| e.into_inner()).push(text);
}

/// Puts back every window [`raise_above_overlays`] made topmost.
pub fn restore_raised() {
    let list = std::mem::take(&mut *RAISED.lock().unwrap_or_else(|e| e.into_inner()));
    for hwnd in list {
        let hwnd = hwnd as HWND;
        if is_alive(hwnd) {
            unsafe { SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
        }
    }
}

fn raise_above_overlays(target: HWND) {
    unsafe { SetWindowPos(target, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    let mut raised = RAISED.lock().unwrap_or_else(|e| e.into_inner());
    if !raised.contains(&(target as isize)) {
        raised.push(target as isize);
    }
}

/// The window covering `point` if it is not the target's program, Aura's own or the shell.
pub fn covering(point: (i32, i32), target: HWND, target_pid: u32) -> Option<HWND> {
    let (root, pid) = window_at(point.0, point.1)?;
    let same = root == target || pid == target_pid;
    (!same && !is_shell_window(root) && !is_own_window(root)).then_some(root)
}

fn pointer_away_from(point: (i32, i32), target: HWND) {
    // The middle of the top edge of the target window is almost never a hover trigger.
    let mut rect = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(target, &mut rect) };
    let (mut x, mut y) = ((rect.left + rect.right) / 2, rect.top + 4);
    if (x - point.0).abs() < 60 && (y - point.1).abs() < 60 {
        x = rect.left + 4;
        y = rect.bottom - 4;
    }
    unsafe { SetCursorPos(x, y) };
}

fn wait_until_gone(point: (i32, i32), target: HWND, target_pid: u32, max: Duration) -> Duration {
    let started = Instant::now();
    while started.elapsed() < max {
        if covering(point, target, target_pid).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    started.elapsed()
}

/// Tries to uncover `point` (up to three rounds). Safe by construction: it waits, moves the
/// pointer, and raises the target window; it never closes or clicks anything.
pub fn clear_cover(point: (i32, i32), target: HWND) -> Outcome {
    let target_pid = process_of(target);
    let mut outcome = Outcome::default();
    for _round in 0..3 {
        let Some(cover) = covering(point, target, target_pid) else {
            outcome.cleared = true;
            break;
        };
        let intruder = describe(cover);
        let kind = classify(&intruder);
        let name = if intruder.title.is_empty() { intruder.class.clone() } else { intruder.title.clone() };
        match kind {
            Kind::Tooltip | Kind::Toast => {
                if kind == Kind::Tooltip {
                    pointer_away_from(point, target);
                }
                let max = if kind == Kind::Toast { Duration::from_secs(8) } else { Duration::from_secs(2) };
                let waited = wait_until_gone(point, target, target_pid, max);
                outcome.steps.push(format!("{} ('{name}', {}) was over the target; {}waited {:.1} s", kind.word(), intruder.process, if kind == Kind::Tooltip { "moved the pointer away and " } else { "" }, waited.as_secs_f32()));
                if covering(point, target, target_pid).is_some() && kind == Kind::Toast {
                    // A toast that will not go away is still only an overlay: lift the target above it.
                    raise_above_overlays(target);
                    outcome.steps.push("it stayed, so the target window was raised above it".to_owned());
                }
            }
            Kind::Overlay => {
                raise_above_overlays(target);
                std::thread::sleep(Duration::from_millis(80));
                outcome.steps.push(format!("{} ('{name}', {}) was over the target; raised the target window above it for this action", kind.word(), intruder.process));
            }
            Kind::AppWindow => {
                outcome.steps.push(format!("{} ('{name}', {}) covers the target and was left alone", kind.word(), intruder.process));
                break;
            }
        }
        if covering(point, target, target_pid).is_none() {
            outcome.cleared = true;
            break;
        }
    }
    if !outcome.steps.is_empty() {
        note(format!("Obstruction: {}.", outcome.steps.join("; ")));
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intruder(class: &str, process: &str, title: &str, size: (i32, i32), topmost: bool, no_activate: bool, tool: bool) -> Intruder {
        Intruder { hwnd: 1, title: title.into(), process: process.into(), class: class.into(), size, topmost, no_activate, tool_window: tool }
    }

    #[test]
    fn the_usual_intruders_are_told_apart() {
        assert_eq!(classify(&intruder("tooltips_class32", "chrome", "", (200, 30), true, true, true)), Kind::Tooltip);
        assert_eq!(classify(&intruder("Windows.UI.Core.CoreWindow", "ShellExperienceHost", "New notification", (396, 110), true, false, false)), Kind::Toast);
        assert_eq!(classify(&intruder("Chrome_WidgetWin_1", "slack", "Slack notification", (360, 90), true, true, true)), Kind::Toast);
        assert_eq!(classify(&intruder("Chrome_WidgetWin_1", "chrome", "", (240, 80), false, true, false)), Kind::Tooltip);
        assert_eq!(classify(&intruder("PetWindow", "desktoppet", "", (128, 128), true, false, true)), Kind::Overlay);
        assert_eq!(classify(&intruder("Notepad", "notepad", "Untitled - Notepad", (800, 600), false, false, false)), Kind::AppWindow);
        assert_eq!(classify(&intruder("#32770", "installer", "Setup", (420, 300), true, false, false)), Kind::AppWindow, "a dialog is a window, not an overlay");
    }

    #[test]
    fn a_point_on_the_target_itself_needs_no_clearing() {
        // Whatever window is at this point counts as the target, so nothing covers it.
        let Some((root, _)) = window_at(20, 20) else { return };
        let outcome = clear_cover((20, 20), root);
        assert!(outcome.cleared && outcome.steps.is_empty());
        let _ = take_notes();
    }
}
