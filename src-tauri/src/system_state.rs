//! Facts about the Windows session that decide whether it is safe to act: is the PC locked or
//! showing a UAC prompt (the secure desktop, which no program may type into), are Sticky Keys,
//! Filter Keys or High Contrast on, did the PC sleep, and is a captured frame black.
//!
//! The accessibility guard switches the *shortcut* that pops up the Sticky Keys dialog (Shift five
//! times) off while a run is going, because the agent presses Shift often; it puts the settings
//! back when the run ends. It never changes the user's saved preferences (no SPIF_UPDATEINIFILE).

use std::mem::size_of;

use windows_sys::Win32::UI::Accessibility::{FILTERKEYS, HCF_HIGHCONTRASTON, HIGHCONTRASTW, STICKYKEYS, TOGGLEKEYS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETFILTERKEYS, SPI_GETHIGHCONTRAST, SPI_GETSTICKYKEYS, SPI_GETTOGGLEKEYS, SPI_SETFILTERKEYS,
    SPI_SETSTICKYKEYS, SPI_SETTOGGLEKEYS,
};

const SKF_STICKYKEYSON: u32 = 0x1;
const SKF_HOTKEYACTIVE: u32 = 0x4;
const SKF_CONFIRMHOTKEY: u32 = 0x8;
const FKF_FILTERKEYSON: u32 = 0x1;
const FKF_HOTKEYACTIVE: u32 = 0x4;
const FKF_CONFIRMHOTKEY: u32 = 0x8;
const TKF_TOGGLEKEYSON: u32 = 0x1;
const TKF_HOTKEYACTIVE: u32 = 0x4;
const TKF_CONFIRMHOTKEY: u32 = 0x8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Accessibility {
    pub sticky_keys_on: bool,
    pub filter_keys_on: bool,
    pub toggle_keys_on: bool,
    pub high_contrast: bool,
    pub sticky_hotkey: bool,
    pub filter_hotkey: bool,
    pub toggle_hotkey: bool,
}

pub fn accessibility() -> Accessibility {
    let mut out = Accessibility::default();
    unsafe {
        let mut sticky: STICKYKEYS = std::mem::zeroed();
        sticky.cbSize = size_of::<STICKYKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETSTICKYKEYS, sticky.cbSize, &mut sticky as *mut _ as *mut _, 0) != 0 {
            out.sticky_keys_on = sticky.dwFlags & SKF_STICKYKEYSON != 0;
            out.sticky_hotkey = sticky.dwFlags & SKF_HOTKEYACTIVE != 0;
        }
        let mut filter: FILTERKEYS = std::mem::zeroed();
        filter.cbSize = size_of::<FILTERKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETFILTERKEYS, filter.cbSize, &mut filter as *mut _ as *mut _, 0) != 0 {
            out.filter_keys_on = filter.dwFlags & FKF_FILTERKEYSON != 0;
            out.filter_hotkey = filter.dwFlags & FKF_HOTKEYACTIVE != 0;
        }
        let mut toggle: TOGGLEKEYS = std::mem::zeroed();
        toggle.cbSize = size_of::<TOGGLEKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETTOGGLEKEYS, toggle.cbSize, &mut toggle as *mut _ as *mut _, 0) != 0 {
            out.toggle_keys_on = toggle.dwFlags & TKF_TOGGLEKEYSON != 0;
            out.toggle_hotkey = toggle.dwFlags & TKF_HOTKEYACTIVE != 0;
        }
        let mut contrast: HIGHCONTRASTW = std::mem::zeroed();
        contrast.cbSize = size_of::<HIGHCONTRASTW>() as u32;
        if SystemParametersInfoW(SPI_GETHIGHCONTRAST, contrast.cbSize, &mut contrast as *mut _ as *mut _, 0) != 0 {
            out.high_contrast = contrast.dwFlags & HCF_HIGHCONTRASTON != 0;
        }
    }
    out
}

impl Accessibility {
    /// A sentence for the model when something here changes how the screen looks or keys behave.
    pub fn prompt_note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.high_contrast {
            parts.push("Windows High Contrast is on, so colours look unusual; rely on text and the numbered elements, not colour.");
        }
        if self.sticky_keys_on {
            parts.push("Sticky Keys is ON: a modifier press stays active until the next key, so press modifiers only as part of a hotkey action.");
        }
        if self.filter_keys_on {
            parts.push("Filter Keys is ON: very short key presses can be ignored, so keys may need to be held a little longer.");
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Turns the pop-up shortcuts of Sticky, Filter and Toggle Keys off for a run and puts them back
/// when dropped.
pub struct ShortcutGuard {
    before: Accessibility,
}

impl ShortcutGuard {
    pub fn engage() -> Self {
        let before = accessibility();
        set_shortcuts(false, false, false, &before);
        Self { before }
    }
}

impl Drop for ShortcutGuard {
    fn drop(&mut self) {
        let before = self.before;
        set_shortcuts(before.sticky_hotkey, before.filter_hotkey, before.toggle_hotkey, &before);
    }
}

fn set_shortcuts(sticky_hotkey: bool, filter_hotkey: bool, toggle_hotkey: bool, current: &Accessibility) {
    // Only touch a setting whose hotkey is currently different from what we want.
    unsafe {
        let mut sticky: STICKYKEYS = std::mem::zeroed();
        sticky.cbSize = size_of::<STICKYKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETSTICKYKEYS, sticky.cbSize, &mut sticky as *mut _ as *mut _, 0) != 0 && (sticky.dwFlags & SKF_HOTKEYACTIVE != 0) != sticky_hotkey {
            sticky.dwFlags = if sticky_hotkey { sticky.dwFlags | SKF_HOTKEYACTIVE | SKF_CONFIRMHOTKEY } else { sticky.dwFlags & !(SKF_HOTKEYACTIVE | SKF_CONFIRMHOTKEY) };
            SystemParametersInfoW(SPI_SETSTICKYKEYS, sticky.cbSize, &mut sticky as *mut _ as *mut _, 0);
        }
        let mut filter: FILTERKEYS = std::mem::zeroed();
        filter.cbSize = size_of::<FILTERKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETFILTERKEYS, filter.cbSize, &mut filter as *mut _ as *mut _, 0) != 0 && (filter.dwFlags & FKF_HOTKEYACTIVE != 0) != filter_hotkey {
            filter.dwFlags = if filter_hotkey { filter.dwFlags | FKF_HOTKEYACTIVE | FKF_CONFIRMHOTKEY } else { filter.dwFlags & !(FKF_HOTKEYACTIVE | FKF_CONFIRMHOTKEY) };
            SystemParametersInfoW(SPI_SETFILTERKEYS, filter.cbSize, &mut filter as *mut _ as *mut _, 0);
        }
        let mut toggle: TOGGLEKEYS = std::mem::zeroed();
        toggle.cbSize = size_of::<TOGGLEKEYS>() as u32;
        if SystemParametersInfoW(SPI_GETTOGGLEKEYS, toggle.cbSize, &mut toggle as *mut _ as *mut _, 0) != 0 && (toggle.dwFlags & TKF_HOTKEYACTIVE != 0) != toggle_hotkey {
            toggle.dwFlags = if toggle_hotkey { toggle.dwFlags | TKF_HOTKEYACTIVE | TKF_CONFIRMHOTKEY } else { toggle.dwFlags & !(TKF_HOTKEYACTIVE | TKF_CONFIRMHOTKEY) };
            SystemParametersInfoW(SPI_SETTOGGLEKEYS, toggle.cbSize, &mut toggle as *mut _ as *mut _, 0);
        }
    }
    let _ = current;
}

/// The name of the desktop that currently receives input ("Default" normally; "Winlogon" for the
/// lock screen or a UAC prompt). `None` when it cannot be opened, which is what a secure desktop
/// does to a normal program.
pub fn input_desktop_name() -> Option<String> {
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME};
    unsafe {
        let desktop = OpenInputDesktop(0, 0, 0x0001 /* DESKTOP_READOBJECTS */);
        if desktop.is_null() {
            return None;
        }
        let mut buffer = [0u16; 128];
        let mut needed = 0u32;
        let ok = GetUserObjectInformationW(desktop, UOI_NAME, buffer.as_mut_ptr() as *mut _, (buffer.len() * 2) as u32, &mut needed);
        CloseDesktop(desktop);
        if ok == 0 {
            return None;
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

/// True while the lock screen or a UAC prompt owns the keyboard and mouse: input sent now would go
/// nowhere (or worse, be dropped silently), so a run must hand over or wait.
pub fn secure_desktop_active() -> bool {
    !matches!(input_desktop_name().as_deref(), Some(name) if name.eq_ignore_ascii_case("default"))
}

/// Seconds since the last keyboard or mouse input by anyone (including this program's own input).
pub fn idle_seconds() -> u64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    unsafe {
        let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut info) == 0 {
            return 0;
        }
        u64::from(GetTickCount().wrapping_sub(info.dwTime)) / 1000
    }
}

/// Detects that the PC slept or was suspended between two calls: wall-clock time jumped far past
/// the monotonic time. Returns the seconds that passed unnoticed (0 when no gap).
pub struct SleepWatch {
    wall: std::time::SystemTime,
    mono: std::time::Instant,
}

impl SleepWatch {
    pub fn new() -> Self {
        Self { wall: std::time::SystemTime::now(), mono: std::time::Instant::now() }
    }

    /// Call from a loop that ticks every few seconds.
    pub fn gap_seconds(&mut self) -> u64 {
        let wall = self.wall.elapsed().unwrap_or_default();
        let mono = self.mono.elapsed();
        self.wall = std::time::SystemTime::now();
        self.mono = std::time::Instant::now();
        // `Instant` does not count time spent suspended on all configurations; wall time does.
        wall.saturating_sub(mono).as_secs()
    }
}

/// A frame whose sampled pixels are all (almost) black. A real desktop is practically never pure
/// black, so this means "the capture returned nothing".
pub fn frame_is_black(rgba: &[u8]) -> bool {
    if rgba.len() < 4 {
        return false;
    }
    let pixels = rgba.len() / 4;
    let step = (pixels / 4000).max(1);
    (0..pixels).step_by(step).all(|i| {
        let p = &rgba[i * 4..i * 4 + 3];
        p[0] < 3 && p[1] < 3 && p[2] < 3
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_frames_are_recognised_and_real_ones_are_not() {
        let black = vec![0u8; 200 * 100 * 4];
        assert!(frame_is_black(&black));
        let mut almost = black.clone();
        for i in (0..almost.len()).step_by(4 * 7) {
            almost[i] = 90; // one bright channel here and there
        }
        assert!(!frame_is_black(&almost));
        assert!(!frame_is_black(&[]));
    }

    #[test]
    fn this_session_is_a_normal_unlocked_desktop() {
        // The tests run in the user's logged-in session.
        assert!(!secure_desktop_active(), "desktop is {:?}", input_desktop_name());
    }

    #[test]
    fn accessibility_can_be_read_and_the_guard_restores_it() {
        let before = accessibility();
        {
            let _guard = ShortcutGuard::engage();
            let during = accessibility();
            assert!(!during.sticky_hotkey && !during.filter_hotkey && !during.toggle_hotkey);
            assert_eq!(during.sticky_keys_on, before.sticky_keys_on, "the guard only touches the pop-up shortcuts");
        }
        assert_eq!(accessibility(), before, "settings are put back");
    }

    #[test]
    fn notes_only_appear_when_something_is_on() {
        assert!(Accessibility::default().prompt_note().is_none());
        let note = Accessibility { high_contrast: true, sticky_keys_on: true, ..Default::default() }.prompt_note().unwrap();
        assert!(note.contains("High Contrast") && note.contains("Sticky Keys"));
    }

    #[test]
    fn a_steady_clock_shows_no_sleep_gap() {
        let mut watch = SleepWatch::new();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(watch.gap_seconds(), 0);
    }
}
