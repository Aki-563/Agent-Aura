//! Detects the human using the computer while the agent is running.
//!
//! The low-level hooks in `killswitch.rs` call `note_user_input` for every *hardware* event
//! (events injected through `SendInput`, including Aura's own, carry an "injected" flag and are
//! ignored). The agent loop then pauses until the user has been quiet for a while, so the two
//! never fight over the mouse.

use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `LLKHF_*` / `LLMHF_*` injected flags (identical bit values for keyboard and mouse hooks
/// for the "lower integrity" flag; the plain injected flag differs).
pub const KEYBOARD_INJECTED_MASK: u32 = 0x0000_0010 | 0x0000_0002;
pub const MOUSE_INJECTED_MASK: u32 = 0x0000_0001 | 0x0000_0002;

/// Mouse movement smaller than this many pixels is treated as jitter, not as the user.
const MOVE_THRESHOLD_PX: i32 = 8;

static LAST_USER_INPUT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Aura's own window (the mini player during a run). Hovering, dragging or clicking it, or typing
/// into it, is the user talking to Aura, not taking the computer back, so it does not pause the run.
static OWN_WINDOW: AtomicIsize = AtomicIsize::new(0);

pub fn set_own_window(hwnd: isize) {
    OWN_WINDOW.store(hwnd, Ordering::SeqCst);
}

/// Whether the screen point lies on Aura's own (visible) window.
pub fn on_own_window(x: i32, y: i32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindowVisible};
    let hwnd = OWN_WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        let mut rect = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
        IsWindowVisible(hwnd) != 0 && GetWindowRect(hwnd, &mut rect) != 0 && x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
    }
}

/// Whether Aura's own window has the keyboard (the steer box is open).
pub fn own_window_has_keyboard() -> bool {
    let hwnd = OWN_WINDOW.load(Ordering::SeqCst);
    hwnd != 0 && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize == hwnd
}

/// Whether `pid` owns the window in front (pop-ups of the app being worked on).
pub fn belongs_to_foreground_app(pid: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut owner) };
    owner != 0 && owner == pid
}

/// Records that the user just used the keyboard or mouse.
pub fn note_user_input() {
    LAST_USER_INPUT_MS.store(now_ms(), Ordering::SeqCst);
}

/// Milliseconds since the user last touched the keyboard or mouse (large if never).
pub fn idle_ms() -> u64 {
    let last = LAST_USER_INPUT_MS.load(Ordering::SeqCst);
    if last == 0 {
        u64::MAX
    } else {
        now_ms().saturating_sub(last)
    }
}

/// Whether a hook event came from real hardware rather than `SendInput`.
pub fn is_hardware_event(flags: u32, injected_mask: u32) -> bool {
    flags & injected_mask == 0
}

/// Called for every hardware mouse-move; counts it as user activity only when the pointer
/// actually travelled a meaningful distance since the last counted position.
pub fn note_mouse_position(x: i32, y: i32) {
    let (last_x, last_y) = (LAST_X.load(Ordering::SeqCst), LAST_Y.load(Ordering::SeqCst));
    if last_x == i32::MIN || (x - last_x).abs() + (y - last_y).abs() >= MOVE_THRESHOLD_PX {
        LAST_X.store(x, Ordering::SeqCst);
        LAST_Y.store(y, Ordering::SeqCst);
        if last_x != i32::MIN && !on_own_window(x, y) {
            note_user_input();
        }
    }
}

/// Test helper: forget any recorded activity.
#[cfg(test)]
pub fn reset_for_tests() {
    LAST_USER_INPUT_MS.store(0, Ordering::SeqCst);
    LAST_X.store(i32::MIN, Ordering::SeqCst);
    LAST_Y.store(i32::MIN, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // These tests share process-wide statics.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn injected_events_are_not_the_user() {
        assert!(is_hardware_event(0, KEYBOARD_INJECTED_MASK));
        assert!(!is_hardware_event(0x10, KEYBOARD_INJECTED_MASK));
        assert!(!is_hardware_event(0x02, KEYBOARD_INJECTED_MASK));
        assert!(is_hardware_event(0, MOUSE_INJECTED_MASK));
        assert!(!is_hardware_event(0x01, MOUSE_INJECTED_MASK));
    }

    #[test]
    fn idle_time_starts_large_and_resets_on_input() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_tests();
        assert_eq!(idle_ms(), u64::MAX);
        note_user_input();
        assert!(idle_ms() < 500);
    }

    #[test]
    fn small_mouse_jitter_is_ignored_but_real_movement_counts() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_tests();
        note_mouse_position(100, 100); // first sample only seeds the position
        assert_eq!(idle_ms(), u64::MAX);
        note_mouse_position(102, 101); // 3 px: jitter
        assert_eq!(idle_ms(), u64::MAX);
        note_mouse_position(160, 140); // real movement
        assert!(idle_ms() < 500);
    }
}
