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

    // ---- Added in Phase 9 (HARNESS_COMPLETION_PLAN.md §4). The existing variants above are
    // unchanged; optional fields the model gives on an existing action name (for example
    // `dwell_ms` on mouse_move) are parsed into one of these instead.
    /// Presses a mouse button and keeps it held into the next steps (until mouse_up or a stop).
    #[serde(rename = "mouse_down")]
    MouseDown {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
    },
    /// Releases a held mouse button, optionally after moving to (x, y).
    #[serde(rename = "mouse_up")]
    MouseUp {
        x: Option<i32>,
        y: Option<i32>,
        #[serde(default)]
        button: MouseButton,
    },
    /// Moves the pointer onto (x, y) and rests there for `dwell_ms` (tooltips, hover menus).
    #[serde(rename = "hover")]
    Hover {
        x: i32,
        y: i32,
        dwell_ms: u64,
    },
    /// One continuous stroke through `points`: press at the first, move through every point at a
    /// steady event rate, release at the last. Drags with waypoints and drawing both use it.
    #[serde(rename = "draw_path")]
    DrawPath {
        points: Vec<(i32, i32)>,
        #[serde(default)]
        button: MouseButton,
        duration_ms: u64,
        #[serde(default)]
        hold_keys: Vec<String>,
        #[serde(default)]
        pointer: PointerKind,
        /// Pen pressure 0..1 (pen only).
        pressure: Option<f32>,
        /// Pen tilt in degrees, -90..90 (pen only).
        tilt_x: Option<i32>,
        tilt_y: Option<i32>,
    },
    /// Wheel scroll in raw wheel units (120 = one notch), optionally with keys held (Ctrl = zoom).
    #[serde(rename = "scroll_fine")]
    ScrollFine {
        x: Option<i32>,
        y: Option<i32>,
        delta: i32,
        #[serde(default)]
        horizontal: bool,
        #[serde(default)]
        hold_keys: Vec<String>,
    },
    /// Typing with an explicit method and/or speed.
    #[serde(rename = "type_with")]
    TypeWith {
        text: String,
        #[serde(default)]
        press_enter: bool,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        method: TypeMethod,
        chars_per_second: Option<f32>,
    },
    /// Presses a key and keeps it held into the next steps (until key_up or a stop).
    #[serde(rename = "key_down")]
    KeyDown {
        key: String,
    },
    #[serde(rename = "key_up")]
    KeyUp {
        key: String,
    },
    /// Holds one key down for `hold_ms`, then releases it.
    #[serde(rename = "hold_key")]
    HoldKey {
        key: String,
        hold_ms: u64,
    },
    /// Asks for the region (physical pixels after scaling) as a zoomed screenshot. No input.
    #[serde(rename = "zoom")]
    Zoom {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    /// Takes a fresh look without acting. No input.
    #[serde(rename = "observe")]
    Observe {},
    /// Reads the clipboard text into the step's result. No input.
    #[serde(rename = "clipboard_read")]
    ClipboardRead {},
    /// Puts text on the clipboard. No input.
    #[serde(rename = "clipboard_write")]
    ClipboardWrite {
        text: String,
    },
    /// Minimize / maximize / restore / close / move / resize / snap a window.
    #[serde(rename = "window_action")]
    WindowAction {
        command: WindowCommand,
        /// Window id or number from the window list, or part of its title; None = the target window.
        window: Option<String>,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<i32>,
        height: Option<i32>,
        side: Option<String>,
    },
    /// Switches to the virtual desktop on the left or right.
    #[serde(rename = "switch_desktop")]
    SwitchDesktop {
        direction: String,
    },
    /// Touch gesture through Windows synthetic pointer injection.
    #[serde(rename = "touch")]
    Touch {
        gesture: TouchGesture,
        x: i32,
        y: i32,
        to_x: Option<i32>,
        to_y: Option<i32>,
        /// Pinch: final finger distance / starting distance (2.0 = spread apart).
        scale: Option<f32>,
        /// Rotate: degrees, clockwise positive.
        angle: Option<f32>,
        duration_ms: u64,
    },
    /// Tiles the given windows (ids, numbers or titles) on the screen.
    #[serde(rename = "layout_windows")]
    LayoutWindows {
        layout: String,
        windows: Vec<String>,
    },
}

/// Which input device a stroke is sent as.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    #[default]
    Mouse,
    Pen,
    Touch,
}

/// How text is typed: `auto` keeps the existing behaviour (keys for short text, paste for long).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TypeMethod {
    #[default]
    Auto,
    /// Real key presses where possible (Unicode packets for the rest), one character at a time.
    Keys,
    /// Unicode packets only.
    Unicode,
    /// Clipboard paste, previous clipboard restored.
    Paste,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowCommand {
    Minimize,
    Maximize,
    Restore,
    Close,
    Move,
    Resize,
    Snap,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TouchGesture {
    Tap,
    DoubleTap,
    LongPress,
    Swipe,
    Pinch,
    Rotate,
}

impl ActionPrimitive {
    /// Every screen point the action uses (physical pixels once scaled). Exhaustive on purpose:
    /// a new variant with coordinates must be handled here, or scaling would pass it through raw.
    pub fn points(&self) -> Vec<(i32, i32)> {
        use ActionPrimitive as A;
        match self {
            A::Click { x, y, .. } | A::DoubleClick { x, y, .. } | A::MouseMove { x, y } | A::MouseDown { x, y, .. } | A::Hover { x, y, .. } => vec![(*x, *y)],
            A::DragAndDrop { from_x, from_y, to_x, to_y, .. } => vec![(*from_x, *from_y), (*to_x, *to_y)],
            A::Scroll { x: Some(x), y: Some(y), .. } | A::ScrollFine { x: Some(x), y: Some(y), .. } | A::MouseUp { x: Some(x), y: Some(y), .. } => vec![(*x, *y)],
            A::DrawPath { points, .. } => points.clone(),
            A::Zoom { x, y, width, height } => vec![(*x, *y), (x + width - 1, y + height - 1)],
            A::Touch { x, y, to_x, to_y, .. } => match (to_x, to_y) {
                (Some(tx), Some(ty)) => vec![(*x, *y), (*tx, *ty)],
                _ => vec![(*x, *y)],
            },
            A::WindowAction { x: Some(x), y: Some(y), .. } => vec![(*x, *y)],
            A::Scroll { .. } | A::ScrollFine { .. } | A::MouseUp { .. } | A::WindowAction { .. } | A::TypeText { .. } | A::TypeWith { .. } | A::Hotkey { .. }
            | A::KeyPress { .. } | A::KeyDown { .. } | A::KeyUp { .. } | A::HoldKey { .. } | A::OpenApp { .. } | A::FocusWindow { .. } | A::OpenUrl { .. }
            | A::Note { .. } | A::AskUser { .. } | A::WaitSettle { .. } | A::Done { .. } | A::Fail { .. } | A::Observe {} | A::ClipboardRead {}
            | A::ClipboardWrite { .. } | A::SwitchDesktop { .. } | A::LayoutWindows { .. } => Vec::new(),
        }
    }

    /// The same action with every point passed through `map` (screenshot -> physical pixels).
    /// Sizes (zoom and window width/height) are mapped as the distance between two mapped points,
    /// which is exact for the affine screenshot transform. Exhaustive for the same reason as `points`.
    pub fn map_points(self, map: &dyn Fn(i32, i32) -> Result<(i32, i32), String>) -> Result<ActionPrimitive, String> {
        use ActionPrimitive as A;
        let opt = |x: Option<i32>, y: Option<i32>| -> Result<(Option<i32>, Option<i32>), String> {
            match (x, y) {
                (Some(x), Some(y)) => map(x, y).map(|(x, y)| (Some(x), Some(y))),
                _ => Ok((x, y)),
            }
        };
        Ok(match self {
            A::Click { x, y, button, clicks, hold_keys } => { let (x, y) = map(x, y)?; A::Click { x, y, button, clicks, hold_keys } }
            A::DoubleClick { x, y, button } => { let (x, y) = map(x, y)?; A::DoubleClick { x, y, button } }
            A::MouseMove { x, y } => { let (x, y) = map(x, y)?; A::MouseMove { x, y } }
            A::MouseDown { x, y, button } => { let (x, y) = map(x, y)?; A::MouseDown { x, y, button } }
            A::Hover { x, y, dwell_ms } => { let (x, y) = map(x, y)?; A::Hover { x, y, dwell_ms } }
            A::DragAndDrop { from_x, from_y, to_x, to_y, duration_ms, hold_keys } => {
                let (from_x, from_y) = map(from_x, from_y)?;
                let (to_x, to_y) = map(to_x, to_y)?;
                A::DragAndDrop { from_x, from_y, to_x, to_y, duration_ms, hold_keys }
            }
            A::Scroll { clicks, x, y, horizontal } => { let (x, y) = opt(x, y)?; A::Scroll { clicks, x, y, horizontal } }
            A::ScrollFine { x, y, delta, horizontal, hold_keys } => { let (x, y) = opt(x, y)?; A::ScrollFine { x, y, delta, horizontal, hold_keys } }
            A::MouseUp { x, y, button } => { let (x, y) = opt(x, y)?; A::MouseUp { x, y, button } }
            A::DrawPath { points, button, duration_ms, hold_keys, pointer, pressure, tilt_x, tilt_y } => A::DrawPath {
                points: points.into_iter().map(|(x, y)| map(x, y)).collect::<Result<_, _>>()?,
                button, duration_ms, hold_keys, pointer, pressure, tilt_x, tilt_y,
            },
            A::Zoom { x, y, width, height } => {
                let (left, top) = map(x, y)?;
                let (right, bottom) = map(x + width.max(1) - 1, y + height.max(1) - 1)?;
                A::Zoom { x: left, y: top, width: right - left + 1, height: bottom - top + 1 }
            }
            A::Touch { gesture, x, y, to_x, to_y, scale, angle, duration_ms } => {
                let (x, y) = map(x, y)?;
                let (to_x, to_y) = opt(to_x, to_y)?;
                A::Touch { gesture, x, y, to_x, to_y, scale, angle, duration_ms }
            }
            A::WindowAction { command, window, x, y, width, height, side } => {
                let (x, y) = opt(x, y)?;
                let size = match (width, height) {
                    (Some(w), Some(h)) => {
                        let (ox, oy) = map(0, 0)?;
                        let (ex, ey) = map(w.max(1) - 1, h.max(1) - 1)?;
                        (Some(ex - ox + 1), Some(ey - oy + 1))
                    }
                    other => other,
                };
                A::WindowAction { command, window, x, y, width: size.0, height: size.1, side }
            }
            other @ (A::TypeText { .. } | A::TypeWith { .. } | A::Hotkey { .. } | A::KeyPress { .. } | A::KeyDown { .. } | A::KeyUp { .. } | A::HoldKey { .. }
            | A::OpenApp { .. } | A::FocusWindow { .. } | A::OpenUrl { .. } | A::Note { .. } | A::AskUser { .. } | A::WaitSettle { .. } | A::Done { .. }
            | A::Fail { .. } | A::Observe {} | A::ClipboardRead {} | A::ClipboardWrite { .. } | A::SwitchDesktop { .. } | A::LayoutWindows { .. }) => other,
        })
    }

    /// True for actions that send no input at all (they only inform the loop).
    pub fn sends_no_input(&self) -> bool {
        matches!(
            self,
            ActionPrimitive::Note { .. } | ActionPrimitive::AskUser { .. } | ActionPrimitive::Done { .. } | ActionPrimitive::Fail { .. }
                | ActionPrimitive::Zoom { .. } | ActionPrimitive::Observe {} | ActionPrimitive::ClipboardRead {} | ActionPrimitive::ClipboardWrite { .. }
        )
    }
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
    /// Held on purpose across steps by `key_down` / `mouse_down` orders. Kept through the cleanup
    /// after each action; released by the matching up order, a stop, or the end of the run.
    sticky_keys: Vec<u16>,
    sticky_buttons: Vec<MouseButton>,
}

static HELD: Mutex<HeldInputs> = Mutex::new(HeldInputs {
    keys: Vec::new(),
    buttons: Vec::new(),
    sticky_keys: Vec::new(),
    sticky_buttons: Vec::new(),
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
        let (mut keys, mut buttons) = {
            let mut held = lock_held();
            let mut keys = std::mem::take(&mut held.keys);
            let mut buttons = std::mem::take(&mut held.buttons);
            keys.append(&mut held.sticky_keys);
            buttons.append(&mut held.sticky_buttons);
            (keys, buttons)
        };
        buttons.dedup();
        keys.dedup();
        for button in buttons.iter().rev() {
            let _ = Self::send_mouse_button(button, false);
        }
        for vk in &keys {
            let _ = Self::send_key(*vk, false);
        }
        Self::release_stuck_physical_state(&keys, &buttons);
    }

    /// Releases what the current action pressed, but keeps deliberate `key_down` / `mouse_down`
    /// holds for the next step. Used as the cleanup after every action.
    pub fn release_transient() {
        let (keys, buttons) = {
            let mut held = lock_held();
            let sticky_keys = held.sticky_keys.clone();
            let sticky_buttons = held.sticky_buttons.clone();
            let keys: Vec<u16> = std::mem::take(&mut held.keys).into_iter().filter(|k| !sticky_keys.contains(k)).collect();
            let buttons: Vec<MouseButton> = std::mem::take(&mut held.buttons).into_iter().filter(|b| !sticky_buttons.contains(b)).collect();
            (keys, buttons)
        };
        for button in buttons.iter().rev() {
            let _ = Self::send_mouse_button(button, false);
        }
        for vk in keys {
            let _ = Self::send_key(vk, false);
        }
    }

    /// Keys and buttons held on purpose across steps (for the prompt and for tests).
    pub fn sticky_holds() -> (Vec<u16>, Vec<MouseButton>) {
        let held = lock_held();
        (held.sticky_keys.clone(), held.sticky_buttons.clone())
    }

    /// After a release, Windows' own idea of the key state is checked for the keys and buttons
    /// Aura itself just let go: one that still reads as down (an up event lost on the way) gets
    /// one more up event. Nothing the user pressed is ever touched, and nothing is checked while
    /// the user is actively typing or clicking.
    fn release_stuck_physical_state(keys: &[u16], buttons: &[MouseButton]) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
        if (keys.is_empty() && buttons.is_empty()) || crate::input::takeover::idle_ms() < 300 {
            return;
        }
        thread::sleep(Duration::from_millis(10));
        let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0;
        for vk in keys {
            if down(*vk) {
                let _ = Self::send_key(*vk, false);
            }
        }
        for button in buttons {
            let vk = match button {
                MouseButton::Left => VK_LBUTTON,
                MouseButton::Right => VK_RBUTTON,
                MouseButton::Middle => VK_MBUTTON,
            };
            if down(vk) {
                let _ = Self::send_mouse_button(button, false);
            }
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

    /// Converts physical screen coordinates to normalized Win32 absolute mouse space (0..65535)
    /// so that Windows lands on exactly that pixel (see `exact_norm`). The CORE.md formula
    /// `round(X * 65535 / (vw - 1))` missed 10 of 3000 pixels by 1 px on a 1920x1080 screen
    /// (`aura-cli pointer-check`, 2026-10-30); this one missed none.
    pub fn normalize_coordinates(x: i32, y: i32, bounds: &ScreenBounds) -> (i32, i32) {
        (Self::exact_norm(x - bounds.x, bounds.width), Self::exact_norm(y - bounds.y, bounds.height))
    }

    /// The normalized value (0..65535) that Windows maps back exactly onto pixel `offset` of a
    /// `size`-pixel virtual desktop. Windows converts with `pixel = floor(norm * size / 65536)`, so
    /// the smallest norm reaching the pixel is `ceil(offset * 65536 / size)`; the old
    /// `round(offset * 65535 / (size - 1))` lands one pixel short on some positions.
    pub fn exact_norm(offset: i32, size: i32) -> i32 {
        if size <= 1 {
            return 0;
        }
        let offset = offset.clamp(0, size - 1) as i64;
        let size = size as i64;
        (((offset * 65536) + size - 1) / size).clamp(0, 65535) as i32
    }

    /// Pixel Windows lands on for a normalized value (inverse of `exact_norm`, for tests and checks).
    pub fn pixel_of_norm(norm: i32, size: i32) -> i32 {
        ((norm as i64 * size as i64) / 65536) as i32
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

    /// Where the pointer really is, in physical pixels.
    pub fn cursor_position() -> Option<(i32, i32)> {
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetPhysicalCursorPos(&mut point) } != 0).then_some((point.x, point.y))
    }

    /// Waits up to `ms` for the pointer to report (x, y); returns where it ended up.
    fn wait_for_cursor_at(x: i32, y: i32, ms: u64) -> Option<(i32, i32)> {
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let now = Self::cursor_position();
            if now == Some((x, y)) || std::time::Instant::now() >= deadline {
                return now;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Moves the pointer, waits a moment so hover effects register, and verifies it is on the exact
    /// pixel (closed loop, 0 px tolerance). A miss is corrected with `SetCursorPos` followed by a
    /// zero-distance move event (so the app still gets its mouse-move message) and checked again,
    /// up to three times. If the pointer still is not there, the action is refused: clicking a
    /// pixel next to the target is worse than not clicking.
    pub fn move_to_confirmed(x: i32, y: i32) -> Result<(), InputError> {
        let bounds = Self::get_screen_bounds();
        Self::validate_bounds(x, y, &bounds)?;
        Self::glide_to(x, y)?;
        Self::move_to(x, y)?;
        let mut landed = Self::wait_for_cursor_at(x, y, 30);
        let mut corrections = 0;
        while landed != Some((x, y)) && corrections < 3 {
            Self::check_killswitch()?;
            corrections += 1;
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y) };
            Self::move_to(x, y)?;
            landed = Self::wait_for_cursor_at(x, y, 30);
        }
        pointer_stats::record(landed.map(|(gx, gy)| (gx - x).abs().max((gy - y).abs())).unwrap_or(-1), corrections);
        if landed != Some((x, y)) {
            return Err(InputError::ActionFailed(format!(
                "the pointer could not be placed exactly at ({x}, {y}); Windows reports {landed:?}"
            )));
        }
        // Hover effects (menus, tooltips) need a moment with the pointer at rest.
        thread::sleep(Duration::from_millis(25));
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
            {
                let mut held = lock_held();
                held.buttons.retain(|b| b != button);
                held.sticky_buttons.retain(|b| b != button);
            }
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
        {
            let mut held = lock_held();
            held.keys.retain(|k| *k != vk);
            held.sticky_keys.retain(|k| *k != vk);
        }
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
        // Deliberate key_down / mouse_down holds survive into the next step; everything else goes up.
        Self::release_transient();
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
            ActionPrimitive::MouseDown { .. }
            | ActionPrimitive::MouseUp { .. }
            | ActionPrimitive::Hover { .. }
            | ActionPrimitive::DrawPath { .. }
            | ActionPrimitive::ScrollFine { .. }
            | ActionPrimitive::TypeWith { .. }
            | ActionPrimitive::KeyDown { .. }
            | ActionPrimitive::KeyUp { .. }
            | ActionPrimitive::HoldKey { .. }
            | ActionPrimitive::Zoom { .. }
            | ActionPrimitive::Observe {}
            | ActionPrimitive::ClipboardRead {}
            | ActionPrimitive::ClipboardWrite { .. }
            | ActionPrimitive::WindowAction { .. }
            | ActionPrimitive::SwitchDesktop { .. }
            | ActionPrimitive::Touch { .. }
            | ActionPrimitive::LayoutWindows { .. } => Self::execute_extended(action),
        }
    }
}

mod ext;
pub use ext::{plan_stroke, precise_sleep, take_report};

/// Closed-loop pointer statistics: how often a move landed exactly, and the worst error seen.
/// Written to the test logs; the 0 px target is checked by `aura-cli pointer-check` and conformance.
pub mod pointer_stats {
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    static MOVES: AtomicU64 = AtomicU64::new(0);
    static EXACT_FIRST_TRY: AtomicU64 = AtomicU64::new(0);
    static CORRECTED: AtomicU64 = AtomicU64::new(0);
    static FAILED: AtomicU64 = AtomicU64::new(0);
    static WORST_PX: AtomicI64 = AtomicI64::new(0);

    /// `error_px` is the final error (-1 when the position could not be read).
    pub fn record(error_px: i32, corrections: u32) {
        MOVES.fetch_add(1, Ordering::Relaxed);
        if error_px == 0 && corrections == 0 {
            EXACT_FIRST_TRY.fetch_add(1, Ordering::Relaxed);
        } else if error_px == 0 {
            CORRECTED.fetch_add(1, Ordering::Relaxed);
        } else {
            FAILED.fetch_add(1, Ordering::Relaxed);
        }
        WORST_PX.fetch_max(error_px as i64, Ordering::Relaxed);
    }

    /// (moves, exact on the first try, exact after correction, failed, worst final error px)
    pub fn snapshot() -> (u64, u64, u64, u64, i64) {
        (
            MOVES.load(Ordering::Relaxed),
            EXACT_FIRST_TRY.load(Ordering::Relaxed),
            CORRECTED.load(Ordering::Relaxed),
            FAILED.load(Ordering::Relaxed),
            WORST_PX.load(Ordering::Relaxed),
        )
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

        let (nx, ny) = InputActuator::normalize_coordinates(960, 540, &bounds);
        assert_eq!((nx, ny), (32768, 32768));
    }

    #[test]
    fn every_pixel_maps_back_exactly_on_many_desktop_sizes() {
        // Single monitors at common scalings, odd sizes, and multi-monitor virtual desktops with
        // negative origins: every pixel must survive pixel -> norm -> pixel unchanged.
        for (x0, y0, w, h) in [(0, 0, 1920, 1080), (0, 0, 1536, 864), (0, 0, 2560, 1440), (0, 0, 3840, 2160), (0, 0, 1366, 768), (0, 0, 1281, 721), (-1920, 0, 3840, 1080), (-1280, -300, 3200, 1440)] {
            let bounds = ScreenBounds { x: x0, y: y0, width: w, height: h };
            for x in x0..x0 + w {
                let (nx, _) = InputActuator::normalize_coordinates(x, y0, &bounds);
                assert_eq!(x0 + InputActuator::pixel_of_norm(nx, w), x, "x {x} on {w} wide desktop at {x0}");
            }
            for y in y0..y0 + h {
                let (_, ny) = InputActuator::normalize_coordinates(x0, y, &bounds);
                assert_eq!(y0 + InputActuator::pixel_of_norm(ny, h), y, "y {y} on {h} high desktop at {y0}");
            }
        }
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
