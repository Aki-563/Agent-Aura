//! Win32 GetCursorInfo polling for IDC_WAIT / IDC_APPSTARTING (Phase 3 implementation)
//!
//! Inspects hardware cursor state to identify whether the OS or foreground
//! application is currently busy / loading before visual frames are captured.

use std::ptr;
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, LoadCursorW, CURSORINFO, CURSOR_SHOWING, HCURSOR, IDC_APPSTARTING,
    IDC_ARROW, IDC_HAND, IDC_IBEAM, IDC_WAIT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCursorType {
    /// Full busy hourglass / spinner (`IDC_WAIT`)
    Wait,
    /// Arrow with background loading spinner (`IDC_APPSTARTING`)
    AppStarting,
    /// Standard pointer (`IDC_ARROW`)
    Arrow,
    /// Text selection cursor (`IDC_IBEAM`)
    IBeam,
    /// Hand pointer / link cursor (`IDC_HAND`)
    Hand,
    /// Custom or application-specific cursor
    Other,
    /// Cursor hidden
    Hidden,
}

pub struct CursorMonitor;

impl CursorMonitor {
    /// Queries the current hardware cursor handle and screen coordinates.
    pub fn get_cursor_info() -> Option<(HCURSOR, POINT, bool)> {
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            flags: 0,
            hCursor: ptr::null_mut(),
            ptScreenPos: POINT { x: 0, y: 0 },
        };

        let success = unsafe { GetCursorInfo(&mut info) };
        if success != 0 {
            let is_showing = (info.flags & CURSOR_SHOWING) != 0;
            Some((info.hCursor, info.ptScreenPos, is_showing))
        } else {
            None
        }
    }

    /// Identifies the current system cursor type.
    pub fn current_cursor_type() -> SystemCursorType {
        let (h_cursor, _, is_showing) = match Self::get_cursor_info() {
            Some(res) => res,
            None => return SystemCursorType::Other,
        };

        if !is_showing {
            return SystemCursorType::Hidden;
        }

        unsafe {
            let h_wait = LoadCursorW(ptr::null_mut(), IDC_WAIT);
            let h_appstarting = LoadCursorW(ptr::null_mut(), IDC_APPSTARTING);
            let h_arrow = LoadCursorW(ptr::null_mut(), IDC_ARROW);
            let h_ibeam = LoadCursorW(ptr::null_mut(), IDC_IBEAM);
            let h_hand = LoadCursorW(ptr::null_mut(), IDC_HAND);

            if h_cursor == h_wait {
                SystemCursorType::Wait
            } else if h_cursor == h_appstarting {
                SystemCursorType::AppStarting
            } else if h_cursor == h_arrow {
                SystemCursorType::Arrow
            } else if h_cursor == h_ibeam {
                SystemCursorType::IBeam
            } else if h_cursor == h_hand {
                SystemCursorType::Hand
            } else {
                SystemCursorType::Other
            }
        }
    }

    /// Checks if the operating system or active application is currently busy.
    ///
    /// Returns `true` if cursor is `IDC_WAIT` or `IDC_APPSTARTING`.
    pub fn is_system_busy() -> bool {
        match Self::current_cursor_type() {
            SystemCursorType::Wait | SystemCursorType::AppStarting => true,
            _ => false,
        }
    }

    /// Yields execution until the cursor transitions out of busy state or timeout occurs.
    ///
    /// Returns `true` if cursor settled to idle, `false` if timed out.
    pub fn wait_for_cursor_idle(max_timeout: Duration, poll_interval: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < max_timeout {
            if !Self::is_system_busy() {
                return true;
            }
            thread::sleep(poll_interval);
        }
        !Self::is_system_busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_info_query() {
        if let Some((h_cursor, pt, showing)) = CursorMonitor::get_cursor_info() {
            println!(
                "Hardware cursor: handle={:?}, pt=({}, {}), showing={}",
                h_cursor, pt.x, pt.y, showing
            );
        }
    }

    #[test]
    fn test_cursor_idle_check() {
        let idle = CursorMonitor::wait_for_cursor_idle(
            Duration::from_millis(50),
            Duration::from_millis(10),
        );
        println!("Cursor is idle: {}", idle);
    }
}
