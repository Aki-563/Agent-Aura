//! Executor for the actions added in Phase 9 (HARNESS_COMPLETION_PLAN.md §4): explicit
//! mouse/key down and up, hover with dwell, smooth strokes (`draw_path`, also drags with
//! waypoints), fine and modifier scrolling, typing with a method or speed, key holds, clipboard,
//! window commands, virtual desktops, touch and pen.
//!
//! Every loop checks the kill switch before each event, and every press has its release on all
//! paths (the per-action cleanup in `execute_action` is the second safety net).

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::input::pointer_inject;
use crate::input::shapes;

/// Text an action produced for the model (clipboard contents, a window's new size, ...).
static REPORT: Mutex<Option<String>> = Mutex::new(None);

fn set_report(text: String) {
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
}

/// Takes the report of the action that just ran, if it produced one.
pub fn take_report() -> Option<String> {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Sleeps with sub-millisecond accuracy where Windows supports a high-resolution waitable timer
/// (plain `sleep` rounds up to the 15.6 ms system tick, which makes strokes uneven).
pub fn precise_sleep(duration: Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, TIMER_ALL_ACCESS};
    if duration.is_zero() {
        return;
    }
    unsafe {
        let timer = CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
        if timer.is_null() {
            thread::sleep(duration);
            return;
        }
        // Negative = relative, in 100 ns units.
        let due = -((duration.as_nanos() / 100) as i64).max(1);
        if SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 0) != 0 {
            WaitForSingleObject(timer, INFINITE);
        } else {
            thread::sleep(duration);
        }
        CloseHandle(timer);
    }
}

/// Waits `ms`, checking the kill switch every 25 ms so long waits and holds stay interruptible.
fn sleep_interruptible(ms: u64) -> Result<(), InputError> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        InputActuator::check_killswitch()?;
        let left = deadline.saturating_duration_since(Instant::now());
        thread::sleep(left.min(Duration::from_millis(25)));
    }
    Ok(())
}

/// Default stroke speed when the model gives no duration: about a quick, steady human hand.
const STROKE_PX_PER_SECOND: f64 = 600.0;
/// Shortest gap between two stroke events (apps drop or coalesce faster input).
const MIN_EVENT_INTERVAL: Duration = Duration::from_millis(3);
/// Largest step between two sent stroke points; apps draw straight segments between them.
const MAX_STROKE_STEP_PX: usize = 6;

/// The points actually sent for a stroke and the time between them.
pub fn plan_stroke(points: &[(i32, i32)], duration_ms: u64) -> (Vec<(i32, i32)>, Duration) {
    let dense = shapes::densify(points);
    if dense.len() <= 1 {
        return (dense, Duration::ZERO);
    }
    let duration = if duration_ms == 0 {
        Duration::from_secs_f64((dense.len() as f64 / STROKE_PX_PER_SECOND).clamp(0.2, 20.0))
    } else {
        Duration::from_millis(duration_ms.clamp(50, 60_000))
    };
    let max_events = (duration.as_secs_f64() / MIN_EVENT_INTERVAL.as_secs_f64()).floor().max(2.0) as usize;
    let stride = dense.len().div_ceil(max_events).clamp(1, MAX_STROKE_STEP_PX);
    let events = shapes::event_points(&dense, stride);
    let interval = (duration / events.len().max(1) as u32).max(MIN_EVENT_INTERVAL);
    (events, interval)
}

impl InputActuator {
    pub(super) fn execute_extended(action: &ActionPrimitive) -> Result<(), InputError> {
        Self::check_killswitch()?;
        match action {
            ActionPrimitive::MouseDown { x, y, button } => {
                Self::move_to_confirmed(*x, *y)?;
                Self::mouse_event(button, true)?;
                lock_held().sticky_buttons.push(button.clone());
                set_report(format!("{button:?} button is held down until mouse_up").to_lowercase());
                Ok(())
            }
            ActionPrimitive::MouseUp { x, y, button } => {
                if let (Some(x), Some(y)) = (x, y) {
                    Self::move_to_confirmed(*x, *y)?;
                }
                Self::mouse_event(button, false)
            }
            ActionPrimitive::Hover { x, y, dwell_ms } => {
                Self::move_to_confirmed(*x, *y)?;
                sleep_interruptible((*dwell_ms).min(10_000))
            }
            ActionPrimitive::DrawPath { points, button, duration_ms, hold_keys, pointer, pressure, tilt_x, tilt_y } => {
                Self::draw_path(points, button, *duration_ms, hold_keys, *pointer, pressure.unwrap_or(0.6), tilt_x.unwrap_or(0), tilt_y.unwrap_or(0))
            }
            ActionPrimitive::ScrollFine { x, y, delta, horizontal, hold_keys } => Self::scroll_fine(*x, *y, *delta, *horizontal, hold_keys),
            ActionPrimitive::TypeWith { text, press_enter, overwrite, method, chars_per_second } => {
                Self::type_with(text, *press_enter, *overwrite, *method, *chars_per_second)
            }
            ActionPrimitive::KeyDown { key } => {
                let vk = Self::resolve_vk(key).ok_or_else(|| InputError::InvalidKey(key.clone()))?;
                Self::key_down(vk)?;
                let mut held = lock_held();
                if !held.sticky_keys.contains(&vk) {
                    held.sticky_keys.push(vk);
                }
                drop(held);
                set_report(format!("{key} is held down until key_up"));
                Ok(())
            }
            ActionPrimitive::KeyUp { key } => Self::key_up_str(key),
            ActionPrimitive::HoldKey { key, hold_ms } => {
                let vk = Self::resolve_vk(key).ok_or_else(|| InputError::InvalidKey(key.clone()))?;
                Self::key_down(vk)?;
                let held = sleep_interruptible((*hold_ms).min(30_000));
                let _ = Self::key_up(vk);
                held
            }
            ActionPrimitive::ClipboardWrite { text } => {
                ClipboardBridge::set_text(text).map_err(InputError::ClipboardError)?;
                set_report(format!("the clipboard now holds {} characters", text.chars().count()));
                Ok(())
            }
            ActionPrimitive::ClipboardRead {} => {
                let text = ClipboardBridge::get_text().unwrap_or_default();
                let shown: String = text.chars().take(400).collect();
                set_report(if text.is_empty() { "the clipboard holds no text".into() } else { format!("the clipboard holds: \"{shown}\"{}", if text.chars().count() > 400 { " (cut)" } else { "" }) });
                Ok(())
            }
            ActionPrimitive::WindowAction { command, window, x, y, width, height, side } => {
                match crate::desktop::windowops::perform(*command, window.as_deref(), *x, *y, *width, *height, side.as_deref()) {
                    Ok(report) => {
                        set_report(report);
                        Ok(())
                    }
                    Err(error) => Err(InputError::ActionFailed(error)),
                }
            }
            ActionPrimitive::LayoutWindows { layout, windows } => match crate::desktop::windowops::arrange(layout, windows) {
                Ok(report) => {
                    set_report(report);
                    Ok(())
                }
                Err(error) => Err(InputError::ActionFailed(error)),
            },
            ActionPrimitive::SwitchDesktop { direction } => {
                let arrow = match direction.trim().to_lowercase().as_str() {
                    "left" | "previous" | "prev" => "left",
                    "right" | "next" => "right",
                    other => return Err(InputError::ActionFailed(format!("switch_desktop direction must be left or right, not '{other}'"))),
                };
                Self::hotkey(&["ctrl", "win", arrow])
            }
            ActionPrimitive::Touch { gesture, x, y, to_x, to_y, scale, angle, duration_ms } => {
                Self::touch(*gesture, *x, *y, to_x.zip(*to_y), scale.unwrap_or(1.0), angle.unwrap_or(0.0), *duration_ms)
            }
            ActionPrimitive::Zoom { .. } | ActionPrimitive::Observe {} => Ok(()),
            other => Err(InputError::ActionFailed(format!("not an extended action: {other:?}"))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_path(points: &[(i32, i32)], button: &MouseButton, duration_ms: u64, hold_keys: &[String], pointer: PointerKind, pressure: f32, tilt_x: i32, tilt_y: i32) -> Result<(), InputError> {
        if points.is_empty() {
            return Err(InputError::ActionFailed("draw_path needs at least one point".into()));
        }
        let bounds = Self::get_screen_bounds();
        for &(x, y) in points {
            Self::validate_bounds(x, y, &bounds)?;
        }
        let (events, interval) = plan_stroke(points, duration_ms);
        match pointer {
            PointerKind::Pen => return pointer_inject::pen_stroke(&events, interval, pressure, tilt_x, tilt_y).map_err(InputError::ActionFailed),
            PointerKind::Touch => return pointer_inject::touch_paths(&[events], interval, Duration::ZERO).map_err(InputError::ActionFailed),
            PointerKind::Mouse => {}
        }
        let first = events[0];
        Self::move_to_confirmed(first.0, first.1)?;
        for key in hold_keys {
            Self::key_down_str(key)?;
        }
        Self::mouse_event(button, true)?;
        let stroke = (|| {
            precise_sleep(Duration::from_millis(40));
            for &(x, y) in events.iter().skip(1) {
                Self::move_to(x, y)?; // checks the kill switch before every event
                precise_sleep(interval);
            }
            let last = *events.last().unwrap_or(&first);
            // Rest on the end point so the app sees the pointer there before the release.
            Self::move_to(last.0, last.1)?;
            precise_sleep(Duration::from_millis(60));
            Ok::<(), InputError>(())
        })();
        let released = Self::mouse_event(button, false);
        for key in hold_keys.iter().rev() {
            let _ = Self::key_up_str(key);
        }
        stroke.and(released)
    }

    fn scroll_fine(x: Option<i32>, y: Option<i32>, delta: i32, horizontal: bool, hold_keys: &[String]) -> Result<(), InputError> {
        if let (Some(x), Some(y)) = (x, y) {
            Self::move_to_confirmed(x, y)?;
        }
        for key in hold_keys {
            Self::key_down_str(key)?;
        }
        thread::sleep(Duration::from_millis(if hold_keys.is_empty() { 0 } else { 30 }));
        let flag = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
        let mut remaining = delta.clamp(-24_000, 24_000);
        let sent = (|| {
            while remaining != 0 {
                let chunk = remaining.clamp(-3 * WHEEL_DELTA, 3 * WHEEL_DELTA);
                remaining -= chunk;
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: chunk as u32, dwFlags: flag, time: 0, dwExtraInfo: 0 } },
                };
                Self::dispatch_inputs(&[input])?;
                if remaining != 0 {
                    thread::sleep(Duration::from_millis(15));
                }
            }
            Ok::<(), InputError>(())
        })();
        for key in hold_keys.iter().rev() {
            let _ = Self::key_up_str(key);
        }
        sent
    }

    fn type_with(text: &str, press_enter: bool, overwrite: bool, method: TypeMethod, chars_per_second: Option<f32>) -> Result<(), InputError> {
        if overwrite {
            Self::hotkey(&["ctrl", "a"])?;
            thread::sleep(Duration::from_millis(20));
            Self::key_press("backspace")?;
            thread::sleep(Duration::from_millis(20));
        }
        let per_char = chars_per_second.filter(|c| *c > 0.0).map(|c| Duration::from_secs_f64(1.0 / c.clamp(0.5, 200.0) as f64));
        match (method, per_char) {
            (TypeMethod::Paste, _) => ClipboardBridge::paste_text(text).map_err(InputError::ClipboardError)?,
            (TypeMethod::Auto, None) => Self::type_text(text, false, false)?,
            (TypeMethod::Unicode, _) => {
                for ch in text.chars() {
                    Self::check_killswitch()?;
                    Self::type_unicode_packet(ch)?;
                    precise_sleep(per_char.unwrap_or(Duration::from_millis(5)));
                }
            }
            (TypeMethod::Keys | TypeMethod::Auto, _) => {
                let mut buffer = [0u8; 4];
                for ch in text.chars() {
                    Self::check_killswitch()?;
                    Self::type_unicode(ch.encode_utf8(&mut buffer))?;
                    if let Some(gap) = per_char {
                        precise_sleep(gap);
                    }
                }
            }
        }
        if press_enter {
            thread::sleep(Duration::from_millis(20));
            Self::key_press("enter")?;
        }
        Ok(())
    }

    /// One character as Unicode packets only (no virtual-key mapping), line breaks as keys.
    fn type_unicode_packet(ch: char) -> Result<(), InputError> {
        match ch {
            '\n' => return Self::key_press("enter"),
            '\r' => return Ok(()),
            '\t' => return Self::key_press("tab"),
            _ => {}
        }
        let mut units = [0u16; 2];
        for &unit in ch.encode_utf16(&mut units).iter() {
            let packet = |flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0, wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
            };
            Self::dispatch_inputs(&[packet(KEYEVENTF_UNICODE)])?;
            Self::dispatch_inputs_unchecked(&[packet(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)])?;
        }
        Ok(())
    }

    fn touch(gesture: TouchGesture, x: i32, y: i32, to: Option<(i32, i32)>, scale: f32, angle: f32, duration_ms: u64) -> Result<(), InputError> {
        let bounds = Self::get_screen_bounds();
        Self::validate_bounds(x, y, &bounds)?;
        if let Some((tx, ty)) = to {
            Self::validate_bounds(tx, ty, &bounds)?;
        }
        let duration = Duration::from_millis(if duration_ms == 0 { 400 } else { duration_ms.clamp(50, 10_000) });
        let steps = (duration.as_millis() / 8).clamp(4, 400) as usize;
        let interval = duration / steps as u32;
        let paths = pointer_inject::gesture_paths(gesture, x, y, to, scale, angle, steps);
        let run = |hold: Duration| pointer_inject::touch_paths(&paths, interval, hold).map_err(InputError::ActionFailed);
        match gesture {
            TouchGesture::Tap => run(Duration::ZERO),
            TouchGesture::DoubleTap => {
                run(Duration::ZERO)?;
                thread::sleep(Duration::from_millis(90));
                run(Duration::ZERO)
            }
            TouchGesture::LongPress => run(duration.max(Duration::from_millis(700))),
            TouchGesture::Swipe | TouchGesture::Pinch | TouchGesture::Rotate => run(Duration::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_keeps_a_steady_rate_and_never_skips_more_than_a_few_pixels() {
        let circle = shapes::to_pixels(&shapes::Shape::Circle { cx: 600.0, cy: 400.0, r: 200.0 }.outline());
        let (events, interval) = plan_stroke(&circle, 0);
        assert!(interval >= MIN_EVENT_INTERVAL);
        assert!(shapes::largest_gap(&events) <= MAX_STROKE_STEP_PX as f64 * 1.5, "gap {}", shapes::largest_gap(&events));
        assert_eq!(events.first(), circle.first());
        assert_eq!(events.last(), circle.last());
        // About 1257 px at 600 px/s: roughly two seconds.
        let total = interval * events.len() as u32;
        assert!(total >= Duration::from_millis(1800) && total <= Duration::from_millis(2600), "{total:?}");
    }

    #[test]
    fn a_short_fast_drag_still_sends_every_few_pixels() {
        let (events, interval) = plan_stroke(&[(100, 100), (400, 100)], 100);
        assert!(shapes::largest_gap(&events) <= MAX_STROKE_STEP_PX as f64);
        assert!(interval >= MIN_EVENT_INTERVAL);
        assert_eq!(events.last(), Some(&(400, 100)));
    }

    #[test]
    fn precise_sleep_is_close_to_the_asked_time() {
        let started = Instant::now();
        for _ in 0..20 {
            precise_sleep(Duration::from_millis(3));
        }
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(60) && took < Duration::from_millis(200), "20 x 3 ms took {took:?}");
    }
}
