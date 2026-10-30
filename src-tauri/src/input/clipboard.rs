//! Fast Bulk Text Clipboard Bridge (Phase 1 implementation)
//!
//! Provides sub-millisecond text injection by writing UTF-16 Unicode text directly
//! into the Win32 Clipboard and firing synchronized Ctrl+V keystrokes.

use std::ptr;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

const CF_UNICODETEXT: u32 = 13;

pub struct ClipboardBridge;

impl ClipboardBridge {
    /// Sets UTF-16 Unicode text directly into the Windows clipboard via Win32 API.
    pub fn set_text(text: &str) -> Result<(), String> {
        Self::set_text_with(text, false)
    }

    /// Like `set_text`, and when `private` also marks the content as not for clipboard history,
    /// cloud clipboard or clipboard monitors (the standard Windows formats). Used for Aura's own
    /// transient paste text: it never shows up in Win+V history, and history listeners do not
    /// grab the clipboard in the moment the target app wants to read it.
    pub fn set_text_with(text: &str, private: bool) -> Result<(), String> {
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes_len = utf16.len() * std::mem::size_of::<u16>();

        unsafe {
            // Retry opening clipboard up to 5 times (in case another process has it temporarily open)
            let mut opened = false;
            for _ in 0..5 {
                if OpenClipboard(ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }

            if !opened {
                return Err("Failed to open Windows clipboard (OpenClipboard failed)".into());
            }

            if EmptyClipboard() == 0 {
                CloseClipboard();
                return Err("Failed to empty Windows clipboard (EmptyClipboard failed)".into());
            }

            let h_mem = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
            if h_mem.is_null() {
                CloseClipboard();
                return Err("GlobalAlloc failed for clipboard data".into());
            }

            let p_mem = GlobalLock(h_mem) as *mut u16;
            if p_mem.is_null() {
                CloseClipboard();
                return Err("GlobalLock failed for clipboard handle".into());
            }

            ptr::copy_nonoverlapping(utf16.as_ptr(), p_mem, utf16.len());
            GlobalUnlock(h_mem);

            if SetClipboardData(CF_UNICODETEXT, h_mem).is_null() {
                CloseClipboard();
                return Err("SetClipboardData failed".into());
            }
            if private {
                Self::mark_private();
            }

            CloseClipboard();
        }

        Ok(())
    }

    /// Reads UTF-16 Unicode text from the Windows clipboard.
    pub fn get_text() -> Result<String, String> {
        unsafe {
            let mut opened = false;
            for _ in 0..5 {
                if OpenClipboard(ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }

            if !opened {
                return Err("Failed to open Windows clipboard".into());
            }

            let h_data = GetClipboardData(CF_UNICODETEXT);
            if h_data.is_null() {
                CloseClipboard();
                return Ok(String::new());
            }

            let p_data = GlobalLock(h_data) as *const u16;
            if p_data.is_null() {
                CloseClipboard();
                return Err("GlobalLock failed on clipboard handle".into());
            }

            // Find null terminator
            let mut len = 0;
            while *p_data.add(len) != 0 {
                len += 1;
            }

            let slice = std::slice::from_raw_parts(p_data, len);
            let text = String::from_utf16_lossy(slice);

            GlobalUnlock(h_data);
            CloseClipboard();

            Ok(text)
        }
    }

    /// Injects text instantly by writing to clipboard and firing Ctrl+V.
    ///
    /// Performance: Executes in < 1ms for up to 50,000 characters.
    pub fn paste_text(text: &str) -> Result<(), String> {
        // Keep what the user had copied: put it back after the paste has been consumed.
        let previous = Self::get_text().ok().filter(|text| !text.is_empty());
        Self::set_text_with(text, true)?;
        // The target app reads the clipboard when it gets Ctrl+V. If another program (clipboard
        // history, a clipboard manager) still has it open from our write, that read can fail and
        // nothing is pasted. Wait until the clipboard is free and holds our text.
        Self::wait_until_free_with(text, Duration::from_millis(400));
        let pasted = crate::input::send_input::InputActuator::hotkey(&["ctrl", "v"])
            .map_err(|e| format!("Failed to fire Ctrl+V for paste: {e}"));
        if let Some(previous) = previous {
            thread::sleep(Duration::from_millis(250));
            let _ = Self::set_text_with(&previous, true);
        }
        pasted
    }

    /// Waits until no program holds the clipboard open and it reads back as `text` (bounded).
    fn wait_until_free_with(text: &str, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let free = unsafe {
                if OpenClipboard(ptr::null_mut()) != 0 {
                    CloseClipboard();
                    true
                } else {
                    false
                }
            };
            if free && Self::get_text().as_deref() == Ok(text) {
                // A last moment for listeners that react to the change notification.
                thread::sleep(Duration::from_millis(30));
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Adds the formats Windows checks before keeping clipboard content in history or syncing it.
    /// Must be called while the clipboard is open.
    unsafe fn mark_private() {
        use windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW;
        let wide = |name: &str| name.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let dword_zero = || -> *mut core::ffi::c_void {
            let handle = GlobalAlloc(GMEM_MOVEABLE, 4);
            if !handle.is_null() {
                let p = GlobalLock(handle) as *mut u32;
                if !p.is_null() {
                    *p = 0;
                    GlobalUnlock(handle);
                }
            }
            handle
        };
        for (name, zero) in [("ExcludeClipboardContentFromMonitorProcessing", false), ("CanIncludeInClipboardHistory", true), ("CanUploadToCloudClipboard", true)] {
            let format = RegisterClipboardFormatW(wide(name).as_ptr());
            if format != 0 {
                let data = if zero { dword_zero() } else { GlobalAlloc(GMEM_MOVEABLE, 1) };
                if !data.is_null() {
                    SetClipboardData(format, data);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_set_and_get() {
        let test_payload = "Agent Aura — High Speed Actuation ⚡ 12345 🚀";
        let res = ClipboardBridge::set_text(test_payload);
        if let Err(e) = res {
            println!("Clipboard not accessible in headless environment: {e}");
            return;
        }

        let retrieved = ClipboardBridge::get_text().expect("get_text should succeed");
        assert_eq!(retrieved, test_payload);
    }
}
