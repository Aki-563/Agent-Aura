//! Excludes Aura's own windows (main window, HUD overlay) from screen capture so the model
//! never sees them, without minimizing, hiding or refocusing anything.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowDisplayAffinity, SetWindowDisplayAffinity};

/// `WDA_EXCLUDEFROMCAPTURE` (Windows 10 2004+): the window is invisible to capture APIs but
/// stays fully visible to the user.
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;

/// Excludes the window from capture and verifies that the setting took effect.
pub fn exclude_from_capture(hwnd: HWND) -> Result<(), String> {
    if hwnd.is_null() {
        return Err("null window handle".to_owned());
    }
    unsafe {
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
            return Err(format!(
                "SetWindowDisplayAffinity failed (Windows 10 2004 or newer is required): error {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        let mut affinity = 0u32;
        if GetWindowDisplayAffinity(hwnd, &mut affinity) == 0 || affinity != WDA_EXCLUDEFROMCAPTURE {
            return Err(format!("display affinity did not stick (value {affinity:#x})"));
        }
    }
    Ok(())
}
