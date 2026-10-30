//! Low-level hardware emergency kill-switch (Phase 1 implementation)
//!
//! Intercepts physical keyboard inputs at the kernel message layer via `WH_KEYBOARD_LL`.
//! When `Escape` or `Ctrl + Pause` is detected, atomic state is tripped in < 1ms,
//! halting all pending input batches and returning control to the human user.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_PAUSE, VK_RCONTROL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN,
};
use crate::input::takeover;

static KILL_SWITCH_TRIGGERED: AtomicBool = AtomicBool::new(false);
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

struct KillSwitchManager {
    thread_handle: Option<JoinHandle<()>>,
}

static MANAGER: Mutex<Option<KillSwitchManager>> = Mutex::new(None);

/// `KBDLLHOOKSTRUCT.flags` bits set when the event came from `SendInput` (any integrity level).
const LLKHF_LOWER_IL_INJECTED: u32 = 0x0000_0002;
const LLKHF_INJECTED: u32 = 0x0000_0010;

/// Decides whether a key-down event is a user-issued emergency stop.
///
/// Events injected by `SendInput` (including Agent Aura's own Esc presses used to close
/// menus and dialogs) must never trip the kill switch, only real hardware input may.
fn is_emergency_stop(vk: u16, flags: u32, ctrl_pressed: bool) -> bool {
    if flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
        return false;
    }
    vk == VK_ESCAPE || (vk == VK_PAUSE && ctrl_pressed)
}

/// Low-level keyboard hook callback procedure.
unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 && (w_param == WM_KEYDOWN as usize || w_param == WM_SYSKEYDOWN as usize) {
        let kbd = *(l_param as *const KBDLLHOOKSTRUCT);
        let vk = kbd.vkCode as u16;
        if takeover::is_hardware_event(kbd.flags, takeover::KEYBOARD_INJECTED_MASK) {
            takeover::note_user_input();
        }
        let ctrl_pressed = vk == VK_PAUSE
            && ((GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000) != 0
                || (GetAsyncKeyState(VK_LCONTROL as i32) as u16 & 0x8000) != 0
                || (GetAsyncKeyState(VK_RCONTROL as i32) as u16 & 0x8000) != 0);

        if is_emergency_stop(vk, kbd.flags, ctrl_pressed) {
            KILL_SWITCH_TRIGGERED.store(true, Ordering::SeqCst);
        } else if takeover::is_hardware_event(kbd.flags, takeover::KEYBOARD_INJECTED_MASK) {
            // Global run controls (work whichever window is in front; injected keys never count).
            let down = |key: u16| (GetAsyncKeyState(key as i32) as u16 & 0x8000) != 0;
            if vk == VK_PAUSE && !ctrl_pressed {
                crate::control::toggle_pause();
            } else if vk == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE
                && down(VK_CONTROL)
                && down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_MENU)
            {
                crate::control::pause("the user is writing a new instruction");
                crate::control::request_steer_box();
            }
        }
    }

    CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
}

/// Low-level mouse hook: records real (non-injected) mouse activity so the agent can pause
/// while the user is driving.
unsafe extern "system" fn low_level_mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let mouse = *(l_param as *const MSLLHOOKSTRUCT);
        if takeover::is_hardware_event(mouse.flags, takeover::MOUSE_INJECTED_MASK) {
            match w_param as u32 {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL => takeover::note_user_input(),
                WM_MOUSEMOVE => takeover::note_mouse_position(mouse.pt.x, mouse.pt.y),
                _ => {}
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
}

pub struct KillSwitch;

impl KillSwitch {
    /// Starts the low-level hook listener on a dedicated background OS thread.
    pub fn start_monitoring() -> Result<(), String> {
        let mut mgr_guard = MANAGER.lock().map_err(|e| e.to_string())?;
        if mgr_guard.is_some() {
            return Ok(()); // Already running
        }

        // Reset kill flag on new monitor start
        KILL_SWITCH_TRIGGERED.store(false, Ordering::SeqCst);

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = thread::spawn(move || {
            let hook: HHOOK = unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_keyboard_proc),
                    std::ptr::null_mut(),
                    0,
                )
            };

            if hook.is_null() {
                let _ = tx.send(Err("SetWindowsHookExW returned null handle".into()));
                return;
            }

            // A second hook on the same thread; if it cannot be installed the kill switch still works
            // and only the "pause when the user takes over" feature is unavailable.
            let mouse_hook: HHOOK = unsafe {
                SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), std::ptr::null_mut(), 0)
            };

            let tid = unsafe { GetCurrentThreadId() };
            HOOK_THREAD_ID.store(tid, Ordering::SeqCst);
            let _ = tx.send(Ok(()));

            // Standard Win32 Message Loop required for WH_KEYBOARD_LL hooks
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                UnhookWindowsHookEx(hook);
                if !mouse_hook.is_null() {
                    UnhookWindowsHookEx(mouse_hook);
                }
            }

            HOOK_THREAD_ID.store(0, Ordering::SeqCst);
        });

        match rx.recv() {
            Ok(Ok(())) => {
                *mgr_guard = Some(KillSwitchManager {
                    thread_handle: Some(handle),
                });
                Ok(())
            }
            Ok(Err(err)) => Err(err),
            Err(e) => Err(format!("Hook initialization thread panicked: {e}")),
        }
    }

    /// Stops the low-level hook and terminates the background message loop thread.
    pub fn stop_monitoring() {
        let mut mgr_guard = match MANAGER.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        if let Some(mut mgr) = mgr_guard.take() {
            let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    PostThreadMessageW(tid, WM_QUIT, 0, 0);
                }
            }

            if let Some(handle) = mgr.thread_handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// Queries whether the emergency stop was triggered by the user (<1ms response).
    #[inline(always)]
    pub fn is_triggered() -> bool {
        KILL_SWITCH_TRIGGERED.load(Ordering::SeqCst)
    }

    /// Manually trips the emergency stop.
    pub fn trigger() {
        KILL_SWITCH_TRIGGERED.store(true, Ordering::SeqCst);
    }

    /// Resets the emergency stop state.
    pub fn reset() {
        KILL_SWITCH_TRIGGERED.store(false, Ordering::SeqCst);
    }
}

/// Convenience export for global check inside actuation loops.
#[inline(always)]
pub fn is_killed() -> bool {
    KillSwitch::is_triggered()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_killswitch_trigger_and_reset() {
        KillSwitch::reset();
        assert!(!KillSwitch::is_triggered());
        assert!(!is_killed());

        KillSwitch::trigger();
        assert!(KillSwitch::is_triggered());
        assert!(is_killed());

        KillSwitch::reset();
        assert!(!KillSwitch::is_triggered());
    }

    #[test]
    fn real_escape_stops_but_injected_escape_does_not() {
        assert!(is_emergency_stop(VK_ESCAPE, 0, false));
        assert!(!is_emergency_stop(VK_ESCAPE, LLKHF_INJECTED, false));
        assert!(!is_emergency_stop(VK_ESCAPE, LLKHF_LOWER_IL_INJECTED, false));
        assert!(!is_emergency_stop(VK_ESCAPE, LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED, false));
    }

    #[test]
    fn ctrl_pause_requires_ctrl_and_real_input() {
        assert!(is_emergency_stop(VK_PAUSE, 0, true));
        assert!(!is_emergency_stop(VK_PAUSE, 0, false));
        assert!(!is_emergency_stop(VK_PAUSE, LLKHF_INJECTED, true));
        assert!(!is_emergency_stop(0x41, 0, true));
    }

    #[test]
    fn test_killswitch_lifecycle() {
        let res = KillSwitch::start_monitoring();
        if let Err(e) = res {
            println!("Low-level hook not permitted in sandbox: {e}");
            return;
        }

        assert!(!KillSwitch::is_triggered());
        KillSwitch::stop_monitoring();
    }
}
