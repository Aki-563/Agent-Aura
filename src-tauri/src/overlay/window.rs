//! Transparent click-through layered overlay window (Phase 2 implementation)
//!
//! Creates a fullscreen, borderless Win32 layered window configured with
//! `WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`
//! to ensure all mouse events fall through 100% unimpeded to underlying applications.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, PostMessageW,
    PostQuitMessage, RegisterClassExW, ShowWindowAsync, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    HTTRANSPARENT, MSG, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, WM_CLOSE, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

static REGISTER_CLASS_ONCE: Once = Once::new();
static CLASS_NAME: &[u16] = &[
    'A' as u16, 'g' as u16, 'e' as u16, 'n' as u16, 't' as u16, 'A' as u16, 'u' as u16,
    'r' as u16, 'a' as u16, 'O' as u16, 'v' as u16, 'e' as u16, 'r' as u16, 'l' as u16,
    'a' as u16, 'y' as u16, 0,
];

/// Window procedure callback ensuring all hit testing passes through.
unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    const WM_NCHITTEST: u32 = 0x0084;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_DESTROY: u32 = 0x0002;

    match msg {
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}

fn register_overlay_class() {
    REGISTER_CLASS_ONCE.call_once(|| {
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: ptr::null_mut(),
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: CLASS_NAME.as_ptr(),
            hIconSm: ptr::null_mut(),
        };

        unsafe {
            RegisterClassExW(&wnd_class);
        }
    });
}

pub struct OverlayWindow {
    hwnd: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    visible: AtomicBool,
}

// HWND is an OS handle; safe to send across threads
unsafe impl Send for OverlayWindow {}
unsafe impl Sync for OverlayWindow {}

impl OverlayWindow {
    /// Queries the full virtual desktop display metrics.
    pub fn query_virtual_screen() -> (i32, i32, i32, i32) {
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

            if vw > 0 && vh > 0 {
                (vx, vy, vw, vh)
            } else {
                (
                    0,
                    0,
                    GetSystemMetrics(SM_CXSCREEN).max(1),
                    GetSystemMetrics(SM_CYSCREEN).max(1),
                )
            }
        }
    }

    /// Creates a transparent, click-through overlay window spanning the full virtual desktop.
    ///
    /// The window lives on its own thread with a message loop, so showing and hiding it from any
    /// other thread (the run loop, the render thread, a terminal tool without a message loop) never
    /// blocks waiting for a thread that does not process messages.
    pub fn new() -> Result<Self, String> {
        let (x, y, width, height) = Self::query_virtual_screen();
        let (sender, receiver) = std::sync::mpsc::channel::<isize>();
        std::thread::spawn(move || {
            register_overlay_class();
            let ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
            let hwnd = unsafe {
                CreateWindowExW(
                    ex_style,
                    CLASS_NAME.as_ptr(),
                    ptr::null(),
                    WS_POPUP,
                    x,
                    y,
                    width,
                    height,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            let _ = sender.send(hwnd as isize);
            if hwnd.is_null() {
                return;
            }
            let mut message: MSG = unsafe { std::mem::zeroed() };
            while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        });
        let hwnd = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "The overlay window thread did not start".to_owned())? as HWND;
        if hwnd.is_null() {
            return Err("Failed to create Win32 overlay window via CreateWindowExW".into());
        }

        Ok(Self {
            hwnd,
            x,
            y,
            width,
            height,
            visible: AtomicBool::new(false),
        })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.x, self.y, self.width, self.height)
    }

    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::SeqCst)
    }

    /// Shows overlay window without taking focus away from user's active application.
    pub fn show(&self) {
        unsafe {
            ShowWindowAsync(self.hwnd, SW_SHOWNOACTIVATE as i32);
        }
        self.visible.store(true, Ordering::SeqCst);
    }

    /// Hides overlay window.
    pub fn hide(&self) {
        unsafe {
            ShowWindowAsync(self.hwnd, SW_HIDE as i32);
        }
        self.visible.store(false, Ordering::SeqCst);
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            // Closing from the window's own thread destroys it and ends that thread's message loop.
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_window_creation_and_bounds() {
        let (vx, vy, vw, vh) = OverlayWindow::query_virtual_screen();
        assert!(vw > 0);
        assert!(vh > 0);

        let window = match OverlayWindow::new() {
            Ok(w) => w,
            Err(e) => {
                println!("Overlay window creation skipped in headless/sandbox: {e}");
                return;
            }
        };

        assert!(!window.hwnd().is_null());
        assert_eq!(window.bounds(), (vx, vy, vw, vh));
        assert!(!window.is_visible());

        window.show();
        assert!(window.is_visible());

        window.hide();
        assert!(!window.is_visible());
    }
}
