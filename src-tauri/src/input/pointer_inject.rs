//! Pen and touch input through Windows synthetic pointer injection
//! (`CreateSyntheticPointerDevice` + `InjectSyntheticPointerInput`, Windows 10 1809 and later).
//!
//! Apps receive real `WM_POINTER*` messages with `pointerType` pen or touch, pressure and tilt,
//! exactly as from a tablet or touch screen. When the system does not support injection (older
//! Windows, some remote sessions) the device cannot be created and the caller gets a clear error
//! instead of a silent no-op.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows_sys::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_NEW,
    POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_NONE, PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN, PT_TOUCH, TOUCH_FLAG_NONE, TOUCH_MASK_CONTACTAREA,
    TOUCH_MASK_PRESSURE,
};

use crate::input::killswitch;

/// Devices are created once and kept: creating one per stroke is slow and Windows caps the count.
struct Devices {
    pen: Option<isize>,
    touch: Option<isize>,
}

static DEVICES: Mutex<Devices> = Mutex::new(Devices { pen: None, touch: None });

const MAX_TOUCH_CONTACTS: u32 = 10;

fn device(pen: bool) -> Result<HSYNTHETICPOINTERDEVICE, String> {
    let mut devices = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    let slot = if pen { &mut devices.pen } else { &mut devices.touch };
    if let Some(handle) = *slot {
        return Ok(handle as HSYNTHETICPOINTERDEVICE);
    }
    let handle = unsafe {
        CreateSyntheticPointerDevice(if pen { PT_PEN } else { PT_TOUCH }, if pen { 1 } else { MAX_TOUCH_CONTACTS }, POINTER_FEEDBACK_DEFAULT)
    };
    if handle.is_null() {
        return Err(format!(
            "{} input is not supported here: Windows could not create a synthetic {} device (needs Windows 10 1809 or later, not available in some remote sessions)",
            if pen { "Pen" } else { "Touch" },
            if pen { "pen" } else { "touch" }
        ));
    }
    *slot = Some(handle as isize);
    Ok(handle)
}

/// One contact (finger or pen tip) in one frame.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub phase: Phase,
    /// 0..1
    pub pressure: f32,
    pub tilt_x: i32,
    pub tilt_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// In range, not touching (pen hover).
    Hover,
    Down,
    Move,
    Up,
    /// Leaves range after an up (pen lifts away).
    Leave,
}

fn flags(phase: Phase, primary: bool, first: bool) -> POINTER_FLAGS {
    let base = match phase {
        Phase::Hover => POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE,
        Phase::Down => POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
        Phase::Move => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
        Phase::Up => POINTER_FLAG_UP | POINTER_FLAG_INRANGE,
        Phase::Leave => POINTER_FLAG_UPDATE,
    };
    base | if primary { POINTER_FLAG_PRIMARY } else { 0 } | if first { POINTER_FLAG_NEW } else { 0 }
}

fn pointer_info(kind: i32, contact: &Contact, primary: bool, first: bool) -> POINTER_INFO {
    let mut info: POINTER_INFO = unsafe { std::mem::zeroed() };
    info.pointerType = kind;
    info.pointerId = contact.id;
    info.pointerFlags = flags(contact.phase, primary, first);
    info.ptPixelLocation = POINT { x: contact.x, y: contact.y };
    info
}

/// Sends one frame for a pen (`pen = true`, exactly one contact) or for touch (1..10 contacts).
pub fn inject(pen: bool, contacts: &[Contact], first_frame: bool) -> Result<(), String> {
    if killswitch::is_killed() && !contacts.iter().all(|c| matches!(c.phase, Phase::Up | Phase::Leave)) {
        return Err("stopped".into());
    }
    let handle = device(pen)?;
    let frames: Vec<POINTER_TYPE_INFO> = contacts
        .iter()
        .enumerate()
        .map(|(index, contact)| {
            let primary = index == 0;
            if pen {
                let mut pen_info: POINTER_PEN_INFO = unsafe { std::mem::zeroed() };
                pen_info.pointerInfo = pointer_info(PT_PEN, contact, primary, first_frame);
                pen_info.penFlags = PEN_FLAG_NONE;
                pen_info.penMask = PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
                pen_info.pressure = if matches!(contact.phase, Phase::Down | Phase::Move) { (contact.pressure.clamp(0.0, 1.0) * 1024.0) as u32 } else { 0 };
                pen_info.tiltX = contact.tilt_x.clamp(-90, 90);
                pen_info.tiltY = contact.tilt_y.clamp(-90, 90);
                POINTER_TYPE_INFO { r#type: PT_PEN, Anonymous: POINTER_TYPE_INFO_0 { penInfo: pen_info } }
            } else {
                let mut touch: POINTER_TOUCH_INFO = unsafe { std::mem::zeroed() };
                touch.pointerInfo = pointer_info(PT_TOUCH, contact, primary, first_frame);
                touch.touchFlags = TOUCH_FLAG_NONE;
                touch.touchMask = TOUCH_MASK_CONTACTAREA | TOUCH_MASK_PRESSURE;
                touch.rcContact = RECT { left: contact.x - 2, top: contact.y - 2, right: contact.x + 2, bottom: contact.y + 2 };
                touch.pressure = (contact.pressure.clamp(0.0, 1.0) * 1024.0) as u32;
                POINTER_TYPE_INFO { r#type: PT_TOUCH, Anonymous: POINTER_TYPE_INFO_0 { touchInfo: touch } }
            }
        })
        .collect();
    let ok = unsafe { InjectSyntheticPointerInput(handle, frames.as_ptr(), frames.len() as u32) };
    if ok == 0 {
        Err(format!("Windows refused the synthetic {} input (error {})", if pen { "pen" } else { "touch" }, unsafe {
            windows_sys::Win32::Foundation::GetLastError()
        }))
    } else {
        Ok(())
    }
}

/// A pen stroke through `points` (physical pixels): hover in, touch down, move, lift, leave.
pub fn pen_stroke(points: &[(i32, i32)], interval: Duration, pressure: f32, tilt_x: i32, tilt_y: i32) -> Result<(), String> {
    let Some(&(x0, y0)) = points.first() else { return Ok(()) };
    let contact = |x, y, phase| Contact { id: 0, x, y, phase, pressure, tilt_x, tilt_y };
    inject(true, &[contact(x0, y0, Phase::Hover)], true)?;
    thread::sleep(Duration::from_millis(20));
    inject(true, &[contact(x0, y0, Phase::Down)], false)?;
    let result = (|| {
        for &(x, y) in points.iter().skip(1) {
            crate::input::send_input::precise_sleep(interval);
            inject(true, &[contact(x, y, Phase::Move)], false)?;
        }
        Ok::<(), String>(())
    })();
    let (xl, yl) = *points.last().unwrap_or(&(x0, y0));
    // The lift is always delivered, even after a stop or an error mid-stroke.
    let _ = inject(true, &[contact(xl, yl, Phase::Up)], false);
    let _ = inject(true, &[contact(xl, yl, Phase::Leave)], false);
    result
}

/// Finger paths, one per contact, all the same length; frames are sent at `interval`.
pub fn touch_paths(paths: &[Vec<(i32, i32)>], interval: Duration, hold_at_start: Duration) -> Result<(), String> {
    let frames = paths.iter().map(Vec::len).min().unwrap_or(0);
    if frames == 0 || paths.len() > MAX_TOUCH_CONTACTS as usize {
        return Err("a touch gesture needs 1 to 10 contacts with at least one point".into());
    }
    let contacts_at = |frame: usize, phase: Phase| -> Vec<Contact> {
        paths.iter().enumerate().map(|(id, path)| Contact { id: id as u32, x: path[frame].0, y: path[frame].1, phase, pressure: 0.5, tilt_x: 0, tilt_y: 0 }).collect()
    };
    inject(false, &contacts_at(0, Phase::Down), true)?;
    let result = (|| {
        // Long press: keep the fingers down and still, refreshing the contact so it is not dropped.
        let mut held = Duration::ZERO;
        while held < hold_at_start {
            crate::input::send_input::precise_sleep(Duration::from_millis(30));
            held += Duration::from_millis(30);
            inject(false, &contacts_at(0, Phase::Move), false)?;
        }
        for frame in 1..frames {
            crate::input::send_input::precise_sleep(interval);
            inject(false, &contacts_at(frame, Phase::Move), false)?;
        }
        Ok::<(), String>(())
    })();
    let _ = inject(false, &contacts_at(frames - 1, Phase::Up), false);
    result
}

/// Paths for a named gesture (pure geometry, physical pixels).
pub fn gesture_paths(gesture: crate::input::send_input::TouchGesture, x: i32, y: i32, to: Option<(i32, i32)>, scale: f32, angle: f32, steps: usize) -> Vec<Vec<(i32, i32)>> {
    use crate::input::send_input::TouchGesture as G;
    let steps = steps.max(2);
    let lerp = |a: (i32, i32), b: (i32, i32), t: f64| ((a.0 as f64 + (b.0 - a.0) as f64 * t).round() as i32, (a.1 as f64 + (b.1 - a.1) as f64 * t).round() as i32);
    match gesture {
        G::Tap | G::DoubleTap | G::LongPress => vec![vec![(x, y)]],
        G::Swipe => {
            let end = to.unwrap_or((x, y));
            vec![(0..=steps).map(|i| lerp((x, y), end, i as f64 / steps as f64)).collect()]
        }
        G::Pinch | G::Rotate => {
            // Two fingers on opposite sides of the centre, 120 px apart to start.
            let start_radius = 60.0;
            let (end_radius, end_angle) = match gesture {
                G::Pinch => (start_radius * scale.clamp(0.1, 10.0) as f64, 0.0),
                _ => (start_radius, (angle as f64).to_radians()),
            };
            (0..2)
                .map(|finger| {
                    let base = if finger == 0 { 0.0 } else { std::f64::consts::PI };
                    (0..=steps)
                        .map(|i| {
                            let t = i as f64 / steps as f64;
                            let r = start_radius + (end_radius - start_radius) * t;
                            let a = base + end_angle * t;
                            ((x as f64 + r * a.cos()).round() as i32, (y as f64 + r * a.sin()).round() as i32)
                        })
                        .collect()
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::send_input::TouchGesture;

    #[test]
    fn pinch_spreads_and_rotate_turns_two_fingers_around_the_centre() {
        let pinch = gesture_paths(TouchGesture::Pinch, 500, 400, None, 2.0, 0.0, 10);
        assert_eq!(pinch.len(), 2);
        let spread = |path: &Vec<(i32, i32)>, i: usize| ((path[i].0 - 500) as f64).hypot((path[i].1 - 400) as f64);
        assert!((spread(&pinch[0], 0) - 60.0).abs() < 1.0 && (spread(&pinch[0], 10) - 120.0).abs() < 1.0);
        let rotate = gesture_paths(TouchGesture::Rotate, 500, 400, None, 1.0, 90.0, 10);
        assert_eq!(rotate[0][10], (500, 460), "finger 1 turned a quarter clockwise (screen y down)");
        let swipe = gesture_paths(TouchGesture::Swipe, 100, 100, Some((400, 100)), 1.0, 0.0, 6);
        assert_eq!(swipe[0].first(), Some(&(100, 100)));
        assert_eq!(swipe[0].last(), Some(&(400, 100)));
    }

    #[test]
    fn flags_mark_down_move_and_up() {
        assert_ne!(flags(Phase::Down, true, true) & POINTER_FLAG_DOWN, 0);
        assert_ne!(flags(Phase::Move, true, false) & POINTER_FLAG_INCONTACT, 0);
        assert_eq!(flags(Phase::Up, true, false) & POINTER_FLAG_INCONTACT, 0);
        assert_eq!(flags(Phase::Leave, false, false) & POINTER_FLAG_INRANGE, 0);
    }
}
