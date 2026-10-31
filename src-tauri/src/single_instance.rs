//! Named locks so only one process does a job per data folder: one app window, one scheduler.
//! A lock is a Windows named mutex that exists while its handle is open, so it disappears with the
//! process, also after a crash. Its name includes a hash of the data folder, so test copies that
//! use their own folder (`LOCALAPPDATA` pointed elsewhere) do not block the user's app.

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

pub struct NamedLock(HANDLE);

// The handle is only closed on drop.
unsafe impl Send for NamedLock {}
unsafe impl Sync for NamedLock {}

impl Drop for NamedLock {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn lock_name(purpose: &str, data_root: &std::path::Path) -> String {
    let hash = Sha256::digest(data_root.to_string_lossy().to_lowercase().as_bytes());
    let short: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("Local\\AgentAura-{purpose}-{short}")
}

/// The lock, or None when another process already holds it.
pub fn try_acquire(purpose: &str, data_root: &std::path::Path) -> Option<NamedLock> {
    let name: Vec<u16> = lock_name(purpose, data_root).encode_utf16().chain(Some(0)).collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // Cannot tell; do not block the app over it.
            return Some(NamedLock(std::ptr::null_mut()));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return None;
        }
        Some(NamedLock(handle))
    }
}

/// Brings the already open Agent Aura window to the front (a second launch does this, then quits).
pub fn show_existing_window(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            return;
        }
        ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_until_the_first_lets_go() {
        let root = std::env::temp_dir().join(format!("aura-lock-{}", std::process::id()));
        let first = try_acquire("test", &root).expect("first holder");
        assert!(try_acquire("test", &root).is_none(), "second holder refused");
        assert!(try_acquire("other", &root).is_some(), "a different job is separate");
        assert!(try_acquire("test", &root.join("x")).is_some(), "a different data folder is separate");
        drop(first);
        assert!(try_acquire("test", &root).is_some(), "free again after the first let go");
    }

    #[test]
    fn names_do_not_depend_on_letter_case() {
        assert_eq!(lock_name("app", std::path::Path::new("C:\\Users\\A\\AppData")), lock_name("app", std::path::Path::new("c:\\users\\a\\appdata")));
    }
}
