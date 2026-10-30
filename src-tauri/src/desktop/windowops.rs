//! Window actions the model can order: minimize, maximize, restore, close, move, resize, snap,
//! and tiling. Every change is read back from Windows and the result says what really happened
//! (an app can refuse a size, a close can be cancelled by a "Save changes?" prompt).

use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed, ShowWindow, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE};

use crate::desktop::layout;
use crate::desktop::target;
use crate::input::send_input::WindowCommand;

/// The window an action names, or the current target window when it names none.
pub fn resolve(window: Option<&str>) -> Result<HWND, String> {
    match window.map(str::trim).filter(|w| !w.is_empty()) {
        Some(name) => crate::desktop::registry::resolve(name)
            .or_else(|| target::find_window_for_reference(name))
            .ok_or_else(|| format!("No open window matches '{name}'. Use an id or number from the window list.")),
        None => target::effective_foreground().ok_or_else(|| "There is no target window.".to_owned()),
    }
}

fn describe(hwnd: HWND) -> String {
    format!("'{}'", target::title_of(hwnd))
}

/// Carries out a window command and returns what was measured afterwards.
pub fn perform(command: WindowCommand, window: Option<&str>, x: Option<i32>, y: Option<i32>, width: Option<i32>, height: Option<i32>, side: Option<&str>) -> Result<String, String> {
    let hwnd = resolve(window)?;
    if target::is_own_window(hwnd) {
        return Err("Aura's own windows cannot be changed.".into());
    }
    if let Some(reason) = target::input_blocked_by_elevation(hwnd) {
        return Err(reason);
    }
    let name = describe(hwnd);
    let settle = || thread::sleep(Duration::from_millis(300));
    match command {
        WindowCommand::Minimize => {
            unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            settle();
            if unsafe { IsIconic(hwnd) } != 0 { Ok(format!("{name} is minimized")) } else { Err(format!("{name} did not minimize")) }
        }
        WindowCommand::Maximize => {
            unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
            settle();
            if unsafe { IsZoomed(hwnd) } != 0 { Ok(format!("{name} is maximized")) } else { Err(format!("{name} did not maximize (the app may not allow it)")) }
        }
        WindowCommand::Restore => {
            unsafe { ShowWindow(hwnd, SW_RESTORE) };
            settle();
            let restored = unsafe { IsIconic(hwnd) == 0 && IsZoomed(hwnd) == 0 };
            if restored { Ok(format!("{name} is restored to its normal size")) } else { Err(format!("{name} is still minimized or maximized")) }
        }
        WindowCommand::Close => {
            target::close_window(hwnd as isize);
            thread::sleep(Duration::from_millis(700));
            if !target::is_alive(hwnd) {
                Ok(format!("{name} closed"))
            } else {
                Ok(format!("{name} is still open: it may be asking whether to save. Look at the screen."))
            }
        }
        WindowCommand::Move | WindowCommand::Resize => {
            let current = target::visible_bounds(hwnd).ok_or("the window is gone")?;
            let (w0, h0) = (current.2 - current.0, current.3 - current.1);
            let (left, top) = (x.unwrap_or(current.0), y.unwrap_or(current.1));
            let (w, h) = (width.unwrap_or(w0).max(50), height.unwrap_or(h0).max(50));
            if command == WindowCommand::Move && x.is_none() && y.is_none() {
                return Err("move needs x and y (where the window's top-left corner should go)".into());
            }
            if command == WindowCommand::Resize && width.is_none() && height.is_none() {
                return Err("resize needs width and/or height".into());
            }
            let placed = layout::place(hwnd, (left, top, left + w, top + h))?;
            Ok(report_placement(&name, &placed))
        }
        WindowCommand::Snap => {
            let side = side.ok_or("snap needs a side: left, right, top, bottom, top_left, top_right, bottom_left or bottom_right")?;
            let area = layout::work_area_of(hwnd).ok_or("could not read the work area")?;
            let rect = layout::snap_rect(area, side).ok_or_else(|| format!("unknown snap side '{side}'"))?;
            let placed = layout::place(hwnd, rect)?;
            Ok(report_placement(&name, &placed))
        }
    }
}

fn report_placement(name: &str, placed: &layout::Placement) -> String {
    let (l, t, r, b) = placed.got;
    if placed.exact(1) {
        format!("{name} now covers ({l}, {t})-({r}, {b})")
    } else if placed.refused_size() {
        format!("{name} kept a larger size than asked (its minimum size): it covers ({l}, {t})-({r}, {b})")
    } else {
        format!("{name} could not be placed exactly: it covers ({l}, {t})-({r}, {b})")
    }
}

/// Tiles the named windows; ids, numbers or titles.
pub fn arrange(layout_name: &str, windows: &[String]) -> Result<String, String> {
    if windows.is_empty() || windows.len() > 9 {
        return Err("layout_windows needs 1 to 9 windows".into());
    }
    let handles: Vec<HWND> = windows.iter().map(|w| resolve(Some(w))).collect::<Result<_, _>>()?;
    let placements = layout::apply(&handles, layout_name, 4)?;
    Ok(handles.iter().zip(&placements).map(|(hwnd, placed)| report_placement(&describe(*hwnd), placed)).collect::<Vec<_>>().join("; "))
}
