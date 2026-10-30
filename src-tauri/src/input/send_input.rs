//! Native Win32 SendInput actuation engine (Phase 1 implementation)
//!
//! Provides sub-millisecond, hardware-accurate mouse and keyboard simulation
//! using Windows `SendInput` with virtual desktop coordinate normalization.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
    VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
    VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_NEXT, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, VkKeyScanW, KEYEVENTF_EXTENDEDKEY, MapVirtualKeyW,
    MAPVK_VK_TO_VSC, GetKeyState, GetKeyboardLayout, VkKeyScanExW, VK_APPS, VK_CAPITAL, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19,
    VK_F20, VK_F21, VK_F22, VK_F23, VK_F24, VK_NUMLOCK, VK_PAUSE, VK_SCROLL, VK_SNAPSHOT,
};

const WHEEL_DELTA: i32 = 120;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::input::clipboard::ClipboardBridge;
use crate::input::killswitch;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MouseButton {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "middle")]
    Middle,
}

impl Default for MouseButton {
    fn default() -> Self {
        Self::Left
    }
}

/// Action Primitive matching Agent Aura's Structured Action Grammar (docs/CORE.md Section 2)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ActionPrimitive {
    #[serde(rename = "click")]
    Click {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default = "default_clicks")]
        clicks: u32,
        #[serde(default)]
        hold_keys: Vec<String>,
    },
    #[serde(rename = "double_click")]
    DoubleClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
    },
    #[serde(rename = "drag_and_drop")]
    DragAndDrop {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        #[serde(default = "default_drag_duration")]
        duration_ms: u64,
        #[serde(default)]
        hold_keys: Vec<String>,
    },
    /// Moves the pointer without clicking (hover menus, tooltips).
    #[serde(rename = "mouse_move")]
    MouseMove {
        x: i32,
        y: i32,
    },
    #[serde(rename = "type_text")]
    TypeText {
        text: String,
        #[serde(default)]
        press_enter: bool,
        #[serde(default)]
        overwrite: bool,
    },
    #[serde(rename = "hotkey")]
    Hotkey {
        keys: Vec<String>,
    },
    #[serde(rename = "key_press")]
    KeyPress {
        key: String,
    },
    #[serde(rename = "scroll")]
    Scroll {
        clicks: i32,
        x: Option<i32>,
        y: Option<i32>,
        #[serde(default)]
        horizontal: bool,
    },
    /// Launches an application by name through Start search (Win, type the name, Enter).
    #[serde(rename = "open_app")]
    OpenApp {
        name: String,
    },
    /// Brings an already open window to the front by (part of) its title or program name.
    #[serde(rename = "focus_window")]
    FocusWindow {
        name: String,
    },
    /// Opens an http(s) URL: in the current browser tab when a browser is in front, otherwise in the default browser.
    #[serde(rename = "open_url")]
    OpenUrl {
        url: String,
    },
    /// Saves a short text the agent wants to remember (for example something to paste later). No input is sent.
    #[serde(rename = "note")]
    Note {
        text: String,
    },
    /// Asks the human for help (a password, a CAPTCHA, a click the agent cannot make). No input is sent.
    #[serde(rename = "ask_user")]
    AskUser {
        question: String,
    },
    #[serde(rename = "wait_settle")]
    WaitSettle {
        #[serde(default = "default_settle_timeout")]
        max_timeout_ms: u64,
        expected_cursor: Option<String>,
    },
    #[serde(rename = "done")]
    Done {
        summary: String,
    },
    #[serde(rename = "fail")]
    Fail {
        reason: String,
    },
}

fn default_clicks() -> u32 {
    1
}

fn default_drag_duration() -> u64 {
    250
}

fn default_settle_timeout() -> u64 {
    1000
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, PartialEq)]
pub enum InputError {
    Killed,
    OutOfBounds { x: i32, y: i32, bounds: ScreenBounds },
    InvalidKey(String),
    SendInputFailed(u32),
    ClipboardError(String),
    ActionFailed(String),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Killed => write!(f, "Emergency kill-switch triggered: input aborted immediately"),
            Self::OutOfBounds { x, y, bounds } => write!(
                f,
                "Coordinate ({}, {}) is out of physical bounds [x: {}, y: {}, w: {}, h: {}]",
                x, y, bounds.x, bounds.y, bounds.width, bounds.height
            ),
            Self::InvalidKey(k) => write!(f, "Invalid key identifier: {}", k),
            Self::SendInputFailed(err) => write!(f, "Win32 SendInput returned error code {}", err),
            Self::ClipboardError(msg) => write!(f, "Clipboard actuation failed: {}", msg),
            Self::ActionFailed(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for InputError {}

/// Keys and mouse buttons that Aura has pressed and not yet released.
/// Releases must always be delivered, even after an emergency stop, otherwise
/// Ctrl/Shift/Win or a mouse button stays logically held on the user's machine.
struct HeldInputs {
    keys: Vec<u16>,
    buttons: Vec<MouseButton>,
}

static HELD: Mutex<HeldInputs> = Mutex::new(HeldInputs {
    keys: Vec::new(),
    buttons: Vec::new(),
});

fn lock_held() -> std::sync::MutexGuard<'static, HeldInputs> {
    HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Navigation keys and right-hand modifiers are "extended" keys. Without the flag Windows may
/// treat them as their numpad twins (Up becomes 8 when NumLock is on).
fn is_extended_vk(vk: u16) -> bool {
    [
        VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT, VK_HOME, VK_END, VK_PRIOR, VK_NEXT, VK_INSERT, VK_DELETE,
        VK_RCONTROL, VK_RMENU, VK_LWIN, VK_RWIN, VK_APPS, VK_SNAPSHOT,
    ]
    .contains(&vk)
}

/// Spoken names for punctuation keys.
fn punctuation_alias(name: &str) -> Option<char> {
    Some(match name {
        "plus" => '+',
        "minus" | "dash" | "hyphen" => '-',
        "comma" => ',',
        "period" | "dot" | "fullstop" => '.',
        "slash" => '/',
        "backslash" => '\\',
        "equals" | "equal" => '=',
        "semicolon" => ';',
        "quote" | "apostrophe" => '\'',
        "backtick" | "grave" => '`',
        "lbracket" | "leftbracket" => '[',
        "rbracket" | "rightbracket" => ']',
        _ => return None,
    })
}

pub struct InputActuator;

impl InputActuator {
    /// Sends INPUT structures without consulting the kill switch. Used only for releases.
    fn dispatch_inputs_unchecked(inputs: &[INPUT]) -> Result<(), InputError> {
        if inputs.is_empty() {
            return Ok(());
        }
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent != inputs.len() as u32 {
            Err(InputError::SendInputFailed(sent))
        } else {
            Ok(())
        }
    }

    /// Releases every key and mouse button Aura still holds. Safe to call at any time,
    /// including after the kill switch has fired.
    pub fn release_all() {
        let (keys, buttons) = {
            let mut held = lock_held();
            (
                std::mem::take(&mut held.keys),
                std::mem::take(&mut held.buttons),
            )
        };
        for button in buttons.iter().rev() {
            let _ = Self::send_mouse_button(button, false);
        }
        for vk in keys {
            let _ = Self::send_key(vk, false);
        }
    }

    fn send_key(vk: u16, down: bool) -> Result<(), InputError> {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
                    dwFlags: (if down { 0 } else { KEYEVENTF_KEYUP })
                        | if is_extended_vk(vk) { KEYEVENTF_EXTENDEDKEY } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        Self::dispatch_inputs_unchecked(&[input])
    }

    fn send_mouse_button(button: &MouseButton, down: bool) -> Result<(), InputError> {
        let flag = match (button, down) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        Self::dispatch_inputs_unchecked(&[input])
    }

    /// Queries the current physical desktop boundaries.
    pub fn get_screen_bounds() -> ScreenBounds {
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

            if vw > 0 && vh > 0 {
                ScreenBounds {
                    x: vx,
                    y: vy,
                    width: vw,
                    height: vh,
                }
            } else {
                ScreenBounds {
                    x: 0,
                    y: 0,
                    width: GetSystemMetrics(SM_CXSCREEN).max(1),
                    height: GetSystemMetrics(SM_CYSCREEN).max(1),
                }
            }
        }
    }

    /// Converts physical screen coordinates to normalized Win32 absolute mouse space (0..65535).
    ///
    /// Formula from docs/CORE.md Section 4.1:
    /// Normalized X = round((X - vx) * 65535 / (vw - 1))
    /// Normalized Y = round((Y - vy) * 65535 / (vh - 1))
    pub fn normalize_coordinates(x: i32, y: i32, bounds: &ScreenBounds) -> (i32, i32) {
        let max_w = (bounds.width - 1).max(1) as f64;
        let max_h = (bounds.height - 1).max(1) as f64;

        let norm_x = (((x - bounds.x) as f64 * 65535.0) / max_w).round() as i32;
        let norm_y = (((y - bounds.y) as f64 * 65535.0) / max_h).round() as i32;

        (norm_x.clamp(0, 65535), norm_y.clamp(0, 65535))
    }

    /// Validates screen bounds as mandated by docs/CORE.md Section 3.2 Bounds Validation Rule.
    pub fn validate_bounds(x: i32, y: i32, bounds: &ScreenBounds) -> Result<(), InputError> {
        if x < bounds.x || x >= bounds.x + bounds.width || y < bounds.y || y >= bounds.y + bounds.height {
            Err(InputError::OutOfBounds {
                x,
                y,
                bounds: bounds.clone(),
            })
        } else {
            Ok(())
        }
    }

    fn check_killswitch() -> Result<(), InputError> {
        if killswitch::is_killed() {
            Err(InputError::Killed)
        } else {
            Ok(())
        }
    }

    /// Dispatches a slice of Win32 INPUT structures atomically.
    fn dispatch_inputs(inputs: &[INPUT]) -> Result<(), InputError> {
        Self::check_killswitch()?;
        if inputs.is_empty() {
            return Ok(());
        }

        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };

        if sent != inputs.len() as u32 {
            Err(InputError::SendInputFailed(sent))
        } else {
            Ok(())
        }
    }

    /// Moves mouse cursor to physical coordinates (x, y).
    pub fn move_to(x: i32, y: i32) -> Result<(), InputError> {
        let bounds = Self::get_screen_bounds();
        Self::validate_bounds(x, y, &bounds)?;
        let (norm_x, norm_y) = Self::normalize_coordinates(x, y, &bounds);

        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: norm_x,
                    dy: norm_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        Self::dispatch_inputs(&[input])
    }

    /// Whether the pointer glides to its target (ease-in-out) instead of jumping there.
    pub fn set_smooth_motion(on: bool) {
        SMOOTH_MOTION.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn smooth_motion_enabled() -> bool {
        SMOOTH_MOTION.load(std::sync::atomic::Ordering::SeqCst) && std::env::var_os("AURA_FAST_MOUSE").is_none()
    }

    /// Moves the pointer to (x, y) along a straight line with a slow start and a slow finish, over a
    /// time that grows with the distance (about 0.17 s for 100 px, 0.45 s across a 1080p screen).
    /// A hard jump looks like a glitch to the person watching and gives hover menus no chance to react.
    pub fn glide_to(x: i32, y: i32) -> Result<(), InputError> {
        if !Self::smooth_motion_enabled() {
            return Ok(());
        }
        let mut start = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut start) } == 0 {
            return Ok(());
        }
        let distance = ((x - start.x) as f64).hypot((y - start.y) as f64);
        let Some(total) = glide_duration(distance) else { return Ok(()) };
        let began = std::time::Instant::now();
        loop {
            Self::check_killswitch()?;
            let t = began.elapsed().as_secs_f64() / total.as_secs_f64();
            if t >= 1.0 {
                break;
            }
            let (px, py) = glide_point((start.x, start.y), (x, y), t);
            Self::move_to(px, py)?;
            thread::sleep(Duration::from_millis(4));
        }
        Ok(())
    }

    /// Moves the pointer, waits a moment so hover effects register, and checks it arrived.
    /// A wrong position is retried once; it is never an error (some sessions report a scaled
    /// cursor position), so the action still goes ahead.
    pub fn move_to_confirmed(x: i32, y: i32) -> Result<(), InputError> {
        let bounds = Self::get_screen_bounds();
        Self::validate_bounds(x, y, &bounds)?;
        Self::glide_to(x, y)?;
        Self::move_to(x, y)?;
        thread::sleep(Duration::from_millis(35));
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        let arrived = unsafe { GetCursorPos(&mut point) != 0 && (point.x - x).abs() <= 2 && (point.y - y).abs() <= 2 };
        if !arrived {
            Self::move_to(x, y)?;
            thread::sleep(Duration::from_millis(35));
        }
        Ok(())
    }

    /// Dispatches mouse button down or up at current position.
    pub fn mouse_event(button: &MouseButton, down: bool) -> Result<(), InputError> {
        if down {
            Self::check_killswitch()?;
            Self::send_mouse_button(button, true)?;
            lock_held().buttons.push(button.clone());
            Ok(())
        } else {
            lock_held().buttons.retain(|held| held != button);
            Self::send_mouse_button(button, false)
        }
    }

    /// Clicks at physical coordinates (x, y) with optional repetition and hold keys.
    pub fn click(
        x: i32,
        y: i32,
        button: &MouseButton,
        clicks: u32,
        hold_keys: &[String],
    ) -> Result<(), InputError> {
        Self::check_killswitch()?;
        Self::move_to_confirmed(x, y)?;

        // Press hold keys
        for key in hold_keys {
            Self::key_down_str(key)?;
        }
        if !hold_keys.is_empty() {
            thread::sleep(Duration::from_millis(25));
        }

        for i in 0..clicks {
            Self::check_killswitch()?;
            Self::mouse_event(button, true)?;
            thread::sleep(Duration::from_millis(25));
            Self::mouse_event(button, false)?;

            if i + 1 < clicks {
                thread::sleep(Duration::from_millis(50));
            }
        }

        // Release hold keys in reverse order
        for key in hold_keys.iter().rev() {
            let _ = Self::key_up_str(key);
        }

        Ok(())
    }

    /// Double clicks at physical coordinates (x, y).
    pub fn double_click(x: i32, y: i32, button: &MouseButton) -> Result<(), InputError> {
        Self::click(x, y, button, 2, &[])
    }

    /// Smooth drag and drop from (from_x, from_y) to (to_x, to_y).
    pub fn drag_and_drop(
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        duration_ms: u64,
        hold_keys: &[String],
    ) -> Result<(), InputError> {
        let bounds = Self::get_screen_bounds();
        Self::validate_bounds(from_x, from_y, &bounds)?;
        Self::validate_bounds(to_x, to_y, &bounds)?;

        Self::move_to_confirmed(from_x, from_y)?;

        for key in hold_keys {
            Self::key_down_str(key)?;
        }

        Self::mouse_event(&MouseButton::Left, true)?;
        thread::sleep(Duration::from_millis(60));
        // Many controls only start a drag after the pointer leaves a small radius.
        let (dx, dy) = ((to_x - from_x) as f64, (to_y - from_y) as f64);
        let distance = dx.hypot(dy);
        if distance > 12.0 {
            let nudge_x = from_x + (dx / distance * 6.0).round() as i32;
            let nudge_y = from_y + (dy / distance * 6.0).round() as i32;
            let _ = Self::move_to(nudge_x, nudge_y);
            thread::sleep(Duration::from_millis(40));
        }

        let steps = (duration_ms / 10).max(16) as usize;
        let step_dur = Duration::from_millis(duration_ms / steps as u64);

        for step in 1..=steps {
            Self::check_killswitch()?;
            let t = step as f64 / steps as f64;
            // Smooth ease-in-out curve
            let smooth_t = t * t * (3.0 - 2.0 * t);

            let curr_x = from_x + ((to_x - from_x) as f64 * smooth_t).round() as i32;
            let curr_y = from_y + ((to_y - from_y) as f64 * smooth_t).round() as i32;

            let (norm_x, norm_y) = Self::normalize_coordinates(curr_x, curr_y, &bounds);
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: norm_x,
                        dy: norm_y,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let _ = Self::dispatch_inputs(&[input]);
            thread::sleep(step_dur);
        }

        // Rest on the destination so the drop target sees the pointer before the button goes up.
        let _ = Self::move_to(to_x, to_y);
        thread::sleep(Duration::from_millis(150));
        Self::mouse_event(&MouseButton::Left, false)?;
        thread::sleep(Duration::from_millis(40));

        for key in hold_keys.iter().rev() {
            let _ = Self::key_up_str(key);
        }

        Ok(())
    }

    /// Scrolls the mouse wheel up (positive) or down (negative).
    pub fn scroll(
        clicks: i32,
        x: Option<i32>,
        y: Option<i32>,
        horizontal: bool,
    ) -> Result<(), InputError> {
        Self::check_killswitch()?;
        if let (Some(px), Some(py)) = (x, y) {
            Self::move_to_confirmed(px, py)?;
        }

        let flag = if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        };

        // Very large wheel deltas are ignored or clipped by some apps; send a few notches at a time.
        let mut remaining = clicks.clamp(-200, 200);
        while remaining != 0 {
            Self::check_killswitch()?;
            let chunk = remaining.clamp(-3, 3);
            remaining -= chunk;
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: (chunk * WHEEL_DELTA) as u32,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            Self::dispatch_inputs(&[input])?;
            if remaining != 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    /// Resolves standard key names to Win32 Virtual Key codes.
    pub fn resolve_vk(key_name: &str) -> Option<u16> {
        let name = key_name.trim().to_lowercase();
        match name.as_str() {
            "enter" | "return" => Some(VK_RETURN),
            "tab" => Some(VK_TAB),
            "space" | "spacebar" => Some(VK_SPACE),
            "backspace" => Some(VK_BACK),
            "delete" | "del" => Some(VK_DELETE),
            "escape" | "esc" => Some(VK_ESCAPE),
            "up" => Some(VK_UP),
            "down" => Some(VK_DOWN),
            "left" => Some(VK_LEFT),
            "right" => Some(VK_RIGHT),
            "home" => Some(VK_HOME),
            "end" => Some(VK_END),
            "pageup" | "pgup" => Some(VK_PRIOR),
            "pagedown" | "pgdn" => Some(VK_NEXT),
            "insert" | "ins" => Some(VK_INSERT),
            "ctrl" | "control" => Some(VK_CONTROL),
            "lctrl" => Some(VK_LCONTROL),
            "rctrl" => Some(VK_RCONTROL),
            "alt" => Some(VK_MENU),
            "lalt" => Some(VK_LMENU),
            "ralt" => Some(VK_RMENU),
            "shift" => Some(VK_SHIFT),
            "lshift" => Some(VK_LSHIFT),
            "rshift" => Some(VK_RSHIFT),
            "win" | "windows" | "cmd" | "super" => Some(VK_LWIN),
            "rwin" => Some(VK_RWIN),
            "f1" => Some(VK_F1),
            "f2" => Some(VK_F2),
            "f3" => Some(VK_F3),
            "f4" => Some(VK_F4),
            "f5" => Some(VK_F5),
            "f6" => Some(VK_F6),
            "f7" => Some(VK_F7),
            "f8" => Some(VK_F8),
            "f9" => Some(VK_F9),
            "f10" => Some(VK_F10),
            "f11" => Some(VK_F11),
            "f12" => Some(VK_F12),
            "f13" => Some(VK_F13),
            "f14" => Some(VK_F14),
            "f15" => Some(VK_F15),
            "f16" => Some(VK_F16),
            "f17" => Some(VK_F17),
            "f18" => Some(VK_F18),
            "f19" => Some(VK_F19),
            "f20" => Some(VK_F20),
            "f21" => Some(VK_F21),
            "f22" => Some(VK_F22),
            "f23" => Some(VK_F23),
            "f24" => Some(VK_F24),
            "arrowup" | "uparrow" | "arrow_up" => Some(VK_UP),
            "arrowdown" | "downarrow" | "arrow_down" => Some(VK_DOWN),
            "arrowleft" | "leftarrow" | "arrow_left" => Some(VK_LEFT),
            "arrowright" | "rightarrow" | "arrow_right" => Some(VK_RIGHT),
            "page_up" | "page up" => Some(VK_PRIOR),
            "page_down" | "page down" => Some(VK_NEXT),
            "capslock" | "caps" | "caps_lock" => Some(VK_CAPITAL),
            "numlock" | "num_lock" => Some(VK_NUMLOCK),
            "scrolllock" | "scroll_lock" => Some(VK_SCROLL),
            "printscreen" | "prtsc" | "prtscr" | "print_screen" => Some(VK_SNAPSHOT),
            "pause" | "break" => Some(VK_PAUSE),
            "menu" | "apps" | "contextmenu" => Some(VK_APPS),
            "option" => Some(VK_MENU),
            _ if punctuation_alias(&name).is_some() => {
                let scan = unsafe { VkKeyScanW(punctuation_alias(&name).unwrap_or(' ') as u16) };
                if scan != -1 {
                    Some((scan & 0xFF) as u16)
                } else {
                    None
                }
            }
            _ => {
                // If it's a single ASCII character, map via VkKeyScanW
                if name.chars().count() == 1 {
                    let ch = name.chars().next().unwrap();
                    let scan = unsafe { VkKeyScanW(ch as u16) };
                    if scan != -1 {
                        Some((scan & 0xFF) as u16)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Dispatches a virtual key down event.
    pub fn key_down(vk: u16) -> Result<(), InputError> {
        Self::check_killswitch()?;
        Self::send_key(vk, true)?;
        let mut held = lock_held();
        if !held.keys.contains(&vk) {
            held.keys.push(vk);
        }
        Ok(())
    }

    /// Dispatches a virtual key up event. Always delivered, even after an emergency stop.
    pub fn key_up(vk: u16) -> Result<(), InputError> {
        lock_held().keys.retain(|held| *held != vk);
        Self::send_key(vk, false)
    }

    pub fn key_down_str(key_name: &str) -> Result<(), InputError> {
        let vk = Self::resolve_vk(key_name).ok_or_else(|| InputError::InvalidKey(key_name.into()))?;
        Self::key_down(vk)
    }

    pub fn key_up_str(key_name: &str) -> Result<(), InputError> {
        let vk = Self::resolve_vk(key_name).ok_or_else(|| InputError::InvalidKey(key_name.into()))?;
        Self::key_up(vk)
    }

    /// Presses and releases a single key.
    pub fn key_press(key_name: &str) -> Result<(), InputError> {
        Self::check_killswitch()?;
        let vk = Self::resolve_vk(key_name).ok_or_else(|| InputError::InvalidKey(key_name.into()))?;
        // A character such as "A" or "!" needs Shift held, which the virtual key alone does not say.
        let needs_shift = key_name.chars().count() == 1
            && key_name.chars().next().is_some_and(|ch| {
                let scan = unsafe { VkKeyScanW(ch as u16) };
                scan != -1 && (scan >> 8) & 1 != 0
            });
        if needs_shift {
            Self::key_down(VK_SHIFT)?;
        }
        let pressed = Self::key_down(vk).and_then(|_| {
            thread::sleep(Duration::from_millis(20));
            Self::key_up(vk)
        });
        if needs_shift {
            let _ = Self::key_up(VK_SHIFT);
        }
        pressed
    }

    /// Dispatches a synchronized hotkey combination (e.g. `["ctrl", "shift", "esc"]`).
    pub fn hotkey(keys: &[&str]) -> Result<(), InputError> {
        Self::check_killswitch()?;
        let mut vks = Vec::with_capacity(keys.len());
        for key in keys {
            let vk = Self::resolve_vk(key).ok_or_else(|| InputError::InvalidKey((*key).into()))?;
            vks.push(vk);
        }

        // Press all keys in sequence
        for &vk in &vks {
            Self::check_killswitch()?;
            Self::key_down(vk)?;
            thread::sleep(Duration::from_millis(25));
        }

        // Some shells (File Explorer, XAML apps) only act on a chord that has been held a moment.
        thread::sleep(Duration::from_millis(70));

        // Release all keys in reverse order
        for &vk in vks.iter().rev() {
            let _ = Self::key_up(vk);
            thread::sleep(Duration::from_millis(10));
        }

        Ok(())
    }

    /// Types one plain ASCII character with real key presses, which every program understands
    /// (Calculator, games and remote sessions ignore Unicode packets for keys such as `+`).
    /// Returns false when the character should go as a Unicode packet instead: it is not ASCII,
    /// needs AltGr, or the window in front uses a layout we cannot map with certainty.
    fn type_char_as_key(ch: char) -> Result<bool, InputError> {
        if !ch.is_ascii() || ch.is_ascii_control() {
            return Ok(false);
        }
        let layout = unsafe {
            let mut process = 0u32;
            let thread = GetWindowThreadProcessId(GetForegroundWindow(), &mut process);
            GetKeyboardLayout(thread)
        };
        // East Asian layouts compose text with an IME; real keys would start a composition.
        let primary_language = (layout as usize & 0x3FF) as u32;
        if matches!(primary_language, 0x04 | 0x11 | 0x12) {
            return Ok(false);
        }
        let scan = unsafe { VkKeyScanExW(ch as u16, layout) };
        if scan == -1 {
            return Ok(false);
        }
        let vk = (scan & 0xFF) as u16;
        let modifiers = (scan >> 8) & 0xFF;
        if modifiers & !1 != 0 {
            return Ok(false);
        }
        let mut shift = modifiers & 1 != 0;
        if ch.is_ascii_alphabetic() && unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0 {
            shift = !shift; // Caps Lock already flips letters
        }
        if shift {
            Self::key_down(VK_SHIFT)?;
        }
        // A short hold: some apps (Notepad, XAML text boxes) occasionally drop a press released instantly.
        let pressed = Self::key_down(vk).and_then(|_| {
            thread::sleep(Duration::from_millis(4));
            Self::key_up(vk)
        });
        if shift {
            let _ = Self::key_up(VK_SHIFT);
        }
        pressed.map(|_| true)
    }

    /// Types text using KEYEVENTF_UNICODE, supporting international characters and emojis.
    pub fn type_unicode(text: &str) -> Result<(), InputError> {
        Self::check_killswitch()?;
        let mut previous_was_cr = false;
        for ch in text.chars() {
            Self::check_killswitch()?;
            // Line breaks and tabs are keys, not characters: apps ignore them as Unicode input.
            match ch {
                '\r' => {
                    Self::key_press("enter")?;
                    previous_was_cr = true;
                    continue;
                }
                '\n' => {
                    if !previous_was_cr {
                        Self::key_press("enter")?;
                    }
                    previous_was_cr = false;
                    continue;
                }
                '\t' => {
                    Self::key_press("tab")?;
                    previous_was_cr = false;
                    continue;
                }
                _ => previous_was_cr = false,
            }
            if Self::type_char_as_key(ch)? {
                thread::sleep(Duration::from_millis(8));
                continue;
            }
            let mut utf16_buf = [0u16; 2];
            let encoded = ch.encode_utf16(&mut utf16_buf);

            for &unit in encoded.iter() {
                let down = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };

                let up = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };

                Self::dispatch_inputs(&[down])?;
                Self::dispatch_inputs(&[up])?;
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    /// Smart text typing: strings > 20 characters automatically use Fast Clipboard Bridge.
    pub fn type_text(text: &str, press_enter: bool, overwrite: bool) -> Result<(), InputError> {
        Self::check_killswitch()?;

        if overwrite {
            // Ctrl+A then Backspace
            Self::hotkey(&["ctrl", "a"])?;
            thread::sleep(Duration::from_millis(20));
            Self::key_press("backspace")?;
            thread::sleep(Duration::from_millis(20));
        }

        if text.chars().count() > 20 {
            // Use Clipboard Bridge for instant injection (<1ms)
            ClipboardBridge::paste_text(text).map_err(InputError::ClipboardError)?;
        } else {
            Self::type_unicode(text)?;
        }

        if press_enter {
            thread::sleep(Duration::from_millis(20));
            Self::key_press("enter")?;
        }

        Ok(())
    }

    /// Brings an existing window to the front and makes it the agent's target.
    pub fn focus_window(name: &str) -> Result<(), InputError> {
        Self::check_killswitch()?;
        let hwnd = crate::desktop::target::find_window_for_reference(name).ok_or_else(|| {
            InputError::ActionFailed(format!(
                "No open window matches '{name}'. Use a number or title from the WINDOWS OPEN list, or open_app to start it."
            ))
        })?;
        if !crate::desktop::target::bring_to_front(hwnd) {
            return Err(InputError::ActionFailed(format!("Could not bring '{name}' to the front.")));
        }
        crate::desktop::target::set_target(Some(hwnd));
        Ok(())
    }

    /// Only plain http(s) URLs are accepted; nothing else is ever handed to the shell.
    pub fn validate_url(url: &str) -> Result<String, InputError> {
        let url = url.trim();
        let lower = url.to_lowercase();
        let plain = url.len() <= 2000 && !url.chars().any(|c| c.is_control() || c == ' ');
        if (lower.starts_with("http://") || lower.starts_with("https://")) && plain {
            Ok(url.to_owned())
        } else {
            Err(InputError::ActionFailed("open_url accepts only http:// or https:// addresses without spaces.".into()))
        }
    }

    pub fn open_url(url: &str) -> Result<(), InputError> {
        let url = Self::validate_url(url)?;
        Self::check_killswitch()?;
        let browser_in_front = crate::desktop::target::effective_foreground()
            .map(crate::desktop::target::process_name_of)
            .is_some_and(|name| crate::desktop::target::is_browser_process(&name));
        if browser_in_front {
            // Reuse the current tab: focus the address bar, paste the address, Enter.
            Self::hotkey(&["ctrl", "l"])?;
            thread::sleep(Duration::from_millis(250));
            return Self::type_text(&url, true, false);
        }
        let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "open ".encode_utf16().collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            )
        };
        if (result as isize) > 32 {
            Ok(())
        } else {
            Err(InputError::ActionFailed(format!("Windows could not open the address (code {})", result as isize)))
        }
    }

    /// Opens an application the way a person would: Windows key, type the name, Enter.
    pub fn open_app(name: &str) -> Result<(), InputError> {
        let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
        let name = name.trim();
        if name.is_empty() {
            return Err(InputError::InvalidKey("open_app needs an application name".into()));
        }
        Self::check_killswitch()?;
        Self::hotkey(&["win"])?;
        thread::sleep(Duration::from_millis(700));
        let front_before = unsafe { GetForegroundWindow() };
        Self::type_unicode(name)?;
        thread::sleep(Duration::from_millis(900));
        Self::key_press("enter")?;
        Self::wait_for_new_window_ready(front_before);
        Ok(())
    }

    /// After launching something: wait (up to a few seconds) for another window to take the
    /// foreground and for it to answer messages, so the next keystroke is not typed into a window
    /// that is still starting up. Never an error: some launches open nothing new.
    fn wait_for_new_window_ready(front_before: windows_sys::Win32::Foundation::HWND) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG};
        let deadline = std::time::Instant::now() + Duration::from_secs(7);
        let mut front = front_before;
        while std::time::Instant::now() < deadline {
            front = unsafe { GetForegroundWindow() };
            if front != front_before && !front.is_null() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if front == front_before || front.is_null() {
            return;
        }
        let ready_deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < ready_deadline {
            let mut result = 0usize;
            let answered = unsafe { SendMessageTimeoutW(front, 0, 0, 0, SMTO_ABORTIFHUNG, 150, &mut result) };
            if answered != 0 {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_millis(300));
    }

    /// Dispatches a single typed action primitive. Whatever the outcome (success, error,
    /// emergency stop mid-action), nothing stays held down afterwards.
    pub fn execute_action(action: &ActionPrimitive) -> Result<(), InputError> {
        let result = Self::execute_action_inner(action);
        Self::release_all();
        result
    }

    fn execute_action_inner(action: &ActionPrimitive) -> Result<(), InputError> {
        Self::check_killswitch()?;
        match action {
            ActionPrimitive::Click {
                x,
                y,
                button,
                clicks,
                hold_keys,
            } => Self::click(*x, *y, button, *clicks, hold_keys),
            ActionPrimitive::DoubleClick { x, y, button } => Self::double_click(*x, *y, button),
            ActionPrimitive::MouseMove { x, y } => Self::move_to_confirmed(*x, *y),
            ActionPrimitive::DragAndDrop {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
                hold_keys,
            } => Self::drag_and_drop(*from_x, *from_y, *to_x, *to_y, *duration_ms, hold_keys),
            ActionPrimitive::TypeText {
                text,
                press_enter,
                overwrite,
            } => Self::type_text(text, *press_enter, *overwrite),
            ActionPrimitive::Hotkey { keys } => {
                let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                Self::hotkey(&key_refs)
            }
            ActionPrimitive::KeyPress { key } => Self::key_press(key),
            ActionPrimitive::Scroll {
                clicks,
                x,
                y,
                horizontal,
            } => Self::scroll(*clicks, *x, *y, *horizontal),
            ActionPrimitive::OpenApp { name } => Self::open_app(name),
            ActionPrimitive::FocusWindow { name } => Self::focus_window(name),
            ActionPrimitive::Note { .. } | ActionPrimitive::AskUser { .. } => Ok(()),
            ActionPrimitive::OpenUrl { url } => Self::open_url(url),
            ActionPrimitive::WaitSettle { max_timeout_ms, .. } => {
                thread::sleep(Duration::from_millis(*max_timeout_ms));
                Ok(())
            }
            ActionPrimitive::Done { .. } | ActionPrimitive::Fail { .. } => Ok(()),
        }
    }
}

/// Whether the pointer glides; tests and `AURA_FAST_MOUSE` turn it off.
static SMOOTH_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Time a glide takes for a given distance in pixels; `None` when the pointer is already there.
pub fn glide_duration(distance: f64) -> Option<Duration> {
    if distance < 6.0 {
        return None;
    }
    Some(Duration::from_millis((140.0 + distance * 0.30).clamp(160.0, 560.0) as u64))
}

/// Ease-in-out ("smootherstep"): zero speed and zero acceleration at both ends.
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// The pointer position `t` (0..1) of the way along a glide.
pub fn glide_point(from: (i32, i32), to: (i32, i32), t: f64) -> (i32, i32) {
    let e = ease_in_out(t);
    (from.0 + ((to.0 - from.0) as f64 * e).round() as i32, from.1 + ((to.1 - from.1) as f64 * e).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_starts_and_ends_gently_and_lands_exactly() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-9);
        // Slow start and slow finish: the first and last 10% of the time cover only ~1% of the way.
        assert!(ease_in_out(0.1) < 0.01 && ease_in_out(0.9) > 0.99);
        // Never goes backwards.
        let mut last = 0.0;
        for i in 0..=100 {
            let value = ease_in_out(i as f64 / 100.0);
            assert!(value >= last);
            last = value;
        }
        assert_eq!(glide_point((100, 200), (900, 600), 0.0), (100, 200));
        assert_eq!(glide_point((100, 200), (900, 600), 1.0), (900, 600));
        assert_eq!(glide_point((100, 200), (900, 600), 0.5), (500, 400));
    }

    #[test]
    fn glide_time_grows_with_distance_within_limits() {
        assert_eq!(glide_duration(3.0), None, "already there: no glide");
        let short = glide_duration(100.0).unwrap();
        let across = glide_duration(1900.0).unwrap();
        assert!(short >= Duration::from_millis(160) && short <= Duration::from_millis(200));
        assert_eq!(across, Duration::from_millis(560), "capped so a long move never drags on");
        assert!(glide_duration(800.0).unwrap() > short);
    }

    #[test]
    fn test_coordinate_normalization() {
        let bounds = ScreenBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };

        let (nx, ny) = InputActuator::normalize_coordinates(0, 0, &bounds);
        assert_eq!((nx, ny), (0, 0));

        let (nx, ny) = InputActuator::normalize_coordinates(1919, 1079, &bounds);
        assert_eq!((nx, ny), (65535, 65535));

        let (nx, ny) = InputActuator::normalize_coordinates(960, 540, &bounds);
        assert!((nx - 32767).abs() <= 100);
        assert!((ny - 32767).abs() <= 100);
    }

    #[test]
    fn test_bounds_validation() {
        let bounds = ScreenBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };

        assert!(InputActuator::validate_bounds(100, 100, &bounds).is_ok());
        assert!(InputActuator::validate_bounds(0, 0, &bounds).is_ok());
        assert!(InputActuator::validate_bounds(1919, 1079, &bounds).is_ok());

        assert!(matches!(
            InputActuator::validate_bounds(-1, 500, &bounds),
            Err(InputError::OutOfBounds { .. })
        ));
        assert!(matches!(
            InputActuator::validate_bounds(1920, 500, &bounds),
            Err(InputError::OutOfBounds { .. })
        ));
        assert!(matches!(
            InputActuator::validate_bounds(500, 1080, &bounds),
            Err(InputError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn only_plain_http_urls_are_accepted() {
        assert!(InputActuator::validate_url("https://example.com/a?b=1").is_ok());
        assert!(InputActuator::validate_url("  http://localhost:8080  ").is_ok());
        for bad in ["file:///C:/Windows/system32/cmd.exe", "javascript:alert(1)", "ms-settings:", "cmd.exe", "https://a.com/x y", "https://a.com/a\nb", "ftp://x.org"] {
            assert!(InputActuator::validate_url(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn test_resolve_vk() {
        assert_eq!(InputActuator::resolve_vk("enter"), Some(VK_RETURN));
        assert_eq!(InputActuator::resolve_vk("ctrl"), Some(VK_CONTROL));
        assert_eq!(InputActuator::resolve_vk("shift"), Some(VK_SHIFT));
        assert_eq!(InputActuator::resolve_vk("esc"), Some(VK_ESCAPE));
        assert_eq!(InputActuator::resolve_vk("f5"), Some(VK_F5));
        assert_eq!(InputActuator::resolve_vk("non_existent_key_xyz"), None);
    }

    #[test]
    fn test_action_primitive_serialization() {
        let json = r#"{"type":"click","x":500,"y":300,"button":"left","clicks":1,"hold_keys":[]}"#;
        let action: ActionPrimitive = serde_json::from_str(json).unwrap();
        match action {
            ActionPrimitive::Click { x, y, button, clicks, .. } => {
                assert_eq!(x, 500);
                assert_eq!(y, 300);
                assert_eq!(button, MouseButton::Left);
                assert_eq!(clicks, 1);
            }
            _ => panic!("Expected Click"),
        }
    }
}
