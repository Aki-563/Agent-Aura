//! Reads and sets the Windows display scale ("Scale" in Settings > Display) of the main monitor,
//! so the harness can be tested at 100, 125, 150, 175 and 200 % without a person changing it.
//!
//! Windows has no documented call for this. Settings itself uses `DisplayConfigSetDeviceInfo` with
//! the private request types -3 (get) and -4 (set), where the scale is given as a step relative to
//! the monitor's recommended scale on the list 100, 125, 150, 175, 200, 225, 250, 300, 350, 400,
//! 450, 500. `ScaleGuard` puts the original scale back when it is dropped.

use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::LUID;

/// The scale steps Windows offers, in percent.
pub const STEPS: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

const GET_DPI_SCALE: i32 = -3;
const SET_DPI_SCALE: i32 = -4;

#[repr(C)]
struct GetScale {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    min_rel: i32,
    cur_rel: i32,
    max_rel: i32,
}

#[repr(C)]
struct SetScale {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    rel: i32,
}

/// What the main monitor allows and uses now, in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleInfo {
    pub current: u32,
    pub recommended: u32,
    pub min: u32,
    pub max: u32,
}

impl ScaleInfo {
    /// The steps this monitor accepts.
    pub fn allowed(&self) -> Vec<u32> {
        STEPS.iter().copied().filter(|s| (self.min..=self.max).contains(s)).collect()
    }
}

/// Converts the relative values Windows reports into percentages. `min_rel` is how many steps
/// 100 % lies below the recommended scale.
pub fn interpret(min_rel: i32, cur_rel: i32, max_rel: i32) -> Option<ScaleInfo> {
    let recommended = min_rel.checked_neg()?;
    let at = |rel: i32| usize::try_from(recommended + rel).ok().and_then(|i| STEPS.get(i)).copied();
    Some(ScaleInfo { current: at(cur_rel)?, recommended: at(0)?, min: at(min_rel)?, max: at(max_rel)? })
}

/// The relative value to send for `percent`, given the recommended scale.
pub fn relative_for(percent: u32, recommended: u32) -> Option<i32> {
    let target = STEPS.iter().position(|s| *s == percent)? as i32;
    let base = STEPS.iter().position(|s| *s == recommended)? as i32;
    Some(target - base)
}

/// Adapter and source id of the first active display path (the main monitor on one-monitor PCs).
fn main_source() -> Result<(LUID, u32), String> {
    unsafe {
        let (mut paths, mut modes) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) != 0 {
            return Err("Windows did not report the display configuration".into());
        }
        let mut path_info: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); paths as usize];
        let mut mode_info: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); modes as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut paths, path_info.as_mut_ptr(), &mut modes, mode_info.as_mut_ptr(), std::ptr::null_mut()) != 0 {
            return Err("Windows did not report the display paths".into());
        }
        let path = path_info.first().ok_or("no active display")?;
        Ok((path.sourceInfo.adapterId, path.sourceInfo.id))
    }
}

pub fn get() -> Result<ScaleInfo, String> {
    let (adapter, id) = main_source()?;
    let mut request = GetScale {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: GET_DPI_SCALE, size: std::mem::size_of::<GetScale>() as u32, adapterId: adapter, id },
        min_rel: 0,
        cur_rel: 0,
        max_rel: 0,
    };
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    if status != 0 {
        return Err(format!("Windows refused to report the display scale (error {status})"));
    }
    interpret(request.min_rel, request.cur_rel, request.max_rel).ok_or_else(|| "Windows reported a display scale outside the known steps".to_owned())
}

/// Sets the main monitor's scale to `percent` (one of the allowed steps).
pub fn set(percent: u32) -> Result<(), String> {
    let info = get()?;
    if !info.allowed().contains(&percent) {
        return Err(format!("{percent}% is not offered for this monitor (allowed: {:?})", info.allowed()));
    }
    let rel = relative_for(percent, info.recommended).ok_or("unknown scale step")?;
    let (adapter, id) = main_source()?;
    let mut request = SetScale {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: SET_DPI_SCALE, size: std::mem::size_of::<SetScale>() as u32, adapterId: adapter, id },
        rel,
    };
    let status = unsafe { DisplayConfigSetDeviceInfo(&mut request.header) };
    if status != 0 {
        return Err(format!("Windows refused to change the display scale (error {status})"));
    }
    Ok(())
}

/// Puts the scale that was active when it was made back when dropped (also after a panic).
pub struct ScaleGuard {
    original: u32,
}

impl ScaleGuard {
    pub fn remember() -> Result<Self, String> {
        Ok(Self { original: get()?.current })
    }

    pub fn original(&self) -> u32 {
        self.original
    }
}

impl Drop for ScaleGuard {
    fn drop(&mut self) {
        if let Err(error) = set(self.original) {
            eprintln!("could not put the display scale back to {}%: {error}", self.original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_steps_become_percentages() {
        // Recommended 125 %: 100 % is one step below, 175 % (the most a 1080p screen allows) two above.
        let info = interpret(-1, 0, 2).unwrap();
        assert_eq!(info, ScaleInfo { current: 125, recommended: 125, min: 100, max: 175 });
        assert_eq!(info.allowed(), vec![100, 125, 150, 175]);
        assert_eq!(interpret(-2, 1, 3).unwrap().current, 175);
        assert_eq!(relative_for(200, 125), Some(3));
        assert_eq!(relative_for(100, 150), Some(-2));
        assert_eq!(relative_for(130, 100), None);
        assert!(interpret(-20, 0, 0).is_none());
    }

    #[test]
    fn the_current_scale_can_be_read() {
        let info = get().expect("display scale");
        assert!(info.allowed().contains(&info.current), "{info:?}");
        assert!((100..=500).contains(&info.recommended));
    }
}
