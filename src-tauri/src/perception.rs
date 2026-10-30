//! Native perception helpers used to ground model intent without another provider.
//! UI Automation covers semantic desktop controls while Windows.Media.Ocr covers
//! rendered text in custom or web interfaces.

use image::{DynamicImage, RgbaImage};
use rightkit_ocr::{BackendPreference, Ocr, OcrOptions};
use serde::{Deserialize, Serialize};
use uiautomation::UIAutomation;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId,
};

const MAX_UIA_CANDIDATES: usize = 120;
const MAX_OCR_CANDIDATES: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowContext {
    pub title: String,
    pub process_id: u32,
    /// The window handle (0 when unknown): UI Automation searches from it so apps that draw in
    /// another process than their frame (Calculator, Settings) are fully visible.
    #[serde(default)]
    pub hwnd: isize,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub dpi: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    UiAutomation,
    Ocr,
    VisualAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingCandidate {
    pub source: CandidateSource,
    pub label: String,
    pub control_type: Option<String>,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub confidence: Option<f32>,
}

impl GroundingCandidate {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

pub fn foreground_window_context() -> WindowContext {
    unsafe {
        // Never report Aura itself as the active window: use the remembered target instead.
        let hwnd = crate::desktop::target::effective_foreground().unwrap_or(GetForegroundWindow());
        if hwnd.is_null() {
            return WindowContext::default();
        }

        let mut title_buf = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
        let title = if title_len > 0 {
            String::from_utf16_lossy(&title_buf[..title_len as usize])
        } else {
            String::new()
        };

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        match crate::desktop::target::visible_bounds(hwnd) {
            Some((left, top, right, bottom)) => {
                rect = RECT { left, top, right, bottom };
            }
            None => {
                let _ = GetWindowRect(hwnd, &mut rect);
            }
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(hwnd, &mut process_id);

        WindowContext {
            title,
            process_id,
            hwnd: hwnd as isize,
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            dpi: query_window_dpi(hwnd),
        }
    }
}

fn query_window_dpi(hwnd: windows_sys::Win32::Foundation::HWND) -> u32 {
    unsafe {
        let user32 = [
            'u' as u16, 's' as u16, 'e' as u16, 'r' as u16, '3' as u16, '2' as u16,
            '.' as u16, 'd' as u16, 'l' as u16, 'l' as u16, 0,
        ];
        let module = GetModuleHandleW(user32.as_ptr());
        if module.is_null() {
            return 96;
        }
        let proc = GetProcAddress(module, b"GetDpiForWindow\0".as_ptr());
        let Some(proc) = proc else {
            return 96;
        };
        let get_dpi: unsafe extern "system" fn(windows_sys::Win32::Foundation::HWND) -> u32 =
            std::mem::transmute(proc);
        get_dpi(hwnd).max(96)
    }
}

pub fn collect_candidates(
    rgba: &[u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    window: &WindowContext,
) -> Vec<GroundingCandidate> {
    // UI Automation and OCR are independent and both slow: run them side by side.
    let process_id = window.process_id;
    let hwnd = window.hwnd;
    std::thread::scope(|scope| {
        let uia = scope.spawn(move || collect_uia_candidates(process_id, hwnd, origin_x, origin_y, width, height));
        let mut text_lines = collect_ocr_candidates(rgba, width, height, origin_x, origin_y);
        text_lines.retain(|line| keep_text_on_active_program(line, window));
        let mut combined = uia.join().unwrap_or_default();
        combined.append(&mut text_lines);
        combined
    })
}

/// What has keyboard focus right now and what it holds: the read-back that tells the model whether
/// its typing or click really landed ("keyboard focus is on edit 'Name' containing 'Jane Doe'").
pub fn focused_element_summary() -> Option<(String, bool)> {
    use uiautomation::patterns::{UITextPattern, UIValuePattern};
    let automation = UIAutomation::new().ok()?;
    let element = automation.get_focused_element().ok()?;
    let name = element.get_name().unwrap_or_default();
    let kind = element.get_control_type().ok().map(|k| format!("{k:?}")).unwrap_or_else(|| "control".to_owned());
    let held = element
        .get_pattern::<UIValuePattern>()
        .ok()
        .and_then(|pattern| pattern.get_value().ok())
        .or_else(|| {
            element
                .get_pattern::<UITextPattern>()
                .ok()
                .and_then(|pattern| pattern.get_document_range().ok())
                .and_then(|range| range.get_text(160).ok())
        })
        .map(|text| text.replace(['\r', '\n'], " ").trim().to_owned())
        .filter(|text| !text.is_empty());
    let takes_text = matches!(kind.to_lowercase().as_str(), "edit" | "document" | "combobox" | "text" | "pane" | "custom" | "group" | "window");
    let mut summary = format!("keyboard focus is on {} '{}'", kind.to_lowercase(), name.trim());
    if let Some(text) = held {
        let shown: String = text.chars().take(120).collect();
        summary.push_str(&format!(" containing '{shown}'"));
    }
    Some((summary, takes_text))
}

/// Phrases a vertical scroll position for the model: "scrolled 40% of the way down; more content
/// below". `None` when no scrollable area is found (the app does not expose one).
pub fn describe_scroll_position(percent: f64, view_size: f64) -> String {
    let down = percent.clamp(0.0, 100.0);
    let seen = view_size.clamp(0.0, 100.0);
    let place = if down <= 1.0 {
        "at the very top".to_owned()
    } else if down >= 99.0 {
        "at the very bottom".to_owned()
    } else {
        format!("{down:.0}% of the way down")
    };
    let more = match (down <= 1.0, down >= 99.0) {
        (true, true) => "",
        (true, false) => "; more content below, scroll down to reach it",
        (false, true) => "; no more content below, scroll up to go back",
        (false, false) => "; more content above and below",
    };
    let size = if seen > 0.0 && seen < 100.0 { format!(" (the window shows about {seen:.0}% of the page)") } else { String::new() };
    format!("scroll position: {place}{more}{size}")
}

/// Every named element UI Automation finds in the window right now, with its screen rectangle: used
/// to check that something the model aimed at a moment ago is still where it was.
pub fn current_named_elements(window: &WindowContext) -> Vec<(String, (i32, i32, i32, i32))> {
    collect_uia_candidates(window.process_id, window.hwnd, -1_000_000, -1_000_000, 4_000_000, 4_000_000)
        .into_iter()
        .map(|c| (c.label, (c.left, c.top, c.right, c.bottom)))
        .collect()
}

/// Net scrolling since the page (window title) last changed, for apps whose scroll position cannot
/// be read (Chrome reports its page as not scrollable). Positive = notches scrolled down.
static SCROLLED: std::sync::Mutex<(String, i32)> = std::sync::Mutex::new((String::new(), 0));

/// Estimated scroll position from what we sent: how far down we are in wheel notches, and whether the
/// last scroll moved anything (a scroll that moves nothing means the end of the page was reached).
pub fn track_scroll(window_title: &str, clicks: i32, moved: bool) -> String {
    let mut state = SCROLLED.lock().unwrap_or_else(|e| e.into_inner());
    if state.0 != window_title {
        *state = (window_title.to_owned(), 0);
    }
    let down = -clicks; // wheel clicks are negative when scrolling down
    if moved {
        state.1 += down;
    }
    let net = state.1;
    let place = if net > 0 { format!("about {net} wheel notches below where this page opened") } else if net < 0 { format!("about {} notches above where this page opened", -net) } else { "back at the position where this page opened".to_owned() };
    if moved {
        format!("scroll position (estimated, the app does not report it): {place}")
    } else {
        let (edge, direction) = if down > 0 { ("bottom", "down") } else { ("top", "up") };
        let detail = if net == 0 { String::new() } else { format!(" ({place})") };
        format!("scroll position: the page did not move, so you are at the {edge} of it{detail}; scrolling {direction} further does nothing")
    }
}

/// Where the scrollable area under the mouse (or holding the keyboard focus) is scrolled to.
pub fn scroll_position_hint() -> Option<String> {
    use uiautomation::patterns::UIScrollPattern;
    use uiautomation::types::Point;
    let automation = UIAutomation::new().ok()?;
    let walker = automation.get_control_view_walker().ok()?;
    let mut starts = Vec::new();
    let mut cursor = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut cursor) } != 0 {
        if let Ok(element) = automation.element_from_point(Point::new(cursor.x, cursor.y)) {
            starts.push(element);
        }
    }
    if let Ok(element) = automation.get_focused_element() {
        starts.push(element);
    }
    let debug = std::env::var_os("AURA_DEBUG_SCROLL").is_some();
    for start in starts {
        let mut current = start;
        for _ in 0..16 {
            if debug {
                let has = current.get_pattern::<UIScrollPattern>().ok().map(|p| format!("vscrollable={:?} percent={:?} view={:?}", p.is_vertically_scrollable(), p.get_vertical_scroll_percent(), p.get_vertical_view_size()));
                eprintln!("scroll walk: {:?} '{}' scroll_pattern={has:?}", current.get_control_type().ok(), current.get_name().unwrap_or_default().chars().take(40).collect::<String>());
            }
            if let Ok(pattern) = current.get_pattern::<UIScrollPattern>() {
                if pattern.is_vertically_scrollable().unwrap_or(false) {
                    if let Ok(percent) = pattern.get_vertical_scroll_percent() {
                        if percent >= 0.0 {
                            let view = pattern.get_vertical_view_size().unwrap_or(100.0);
                            return Some(describe_scroll_position(percent, view));
                        }
                    }
                }
            }
            match walker.get_parent(&current) {
                Ok(parent) => current = parent,
                Err(_) => break,
            }
        }
    }
    None
}

/// Text read from the whole screen includes other windows behind the active one; clicking it would
/// hit the wrong window. Keep lines that sit on the active window or on another window of the same
/// program (its pop-up menus and drop-downs).
fn keep_text_on_active_program(line: &GroundingCandidate, window: &WindowContext) -> bool {
    if window.process_id == 0 {
        return true;
    }
    let center_x = (line.left + line.right) / 2;
    let center_y = (line.top + line.bottom) / 2;
    // Ask Windows which window is really on top at the text's position: the active window or a
    // pop-up of the same program counts, anything else (another app, the desktop) does not.
    match crate::desktop::target::window_at(center_x, center_y) {
        Some((root, pid)) => root as isize == window.hwnd || pid == window.process_id || crate::desktop::target::is_shell_window(root),
        None => true,
    }
}

fn collect_uia_candidates(
    process_id: u32,
    hwnd: isize,
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
) -> Vec<GroundingCandidate> {
    if process_id == 0 {
        return Vec::new();
    }
    let Ok(automation) = UIAutomation::new() else {
        return Vec::new();
    };
    // Search below the window itself first: its tree includes content hosted from other
    // processes. Fall back to every element of the window's process.
    let from_window = (hwnd != 0)
        .then(|| automation.element_from_handle(uiautomation::types::Handle::from(hwnd)).ok())
        .flatten()
        .and_then(|window| automation.create_matcher().from(window).depth(14).timeout(0).find_all().ok())
        .filter(|found| !found.is_empty());
    let elements = match from_window {
        Some(found) => found,
        None => {
            let Ok(root) = automation.get_root_element() else {
                return Vec::new();
            };
            let Ok(found) = automation
                .create_matcher()
                .from(root)
                .process_id(process_id)
                .depth(14)
                .timeout(0)
                .find_all()
            else {
                return Vec::new();
            };
            found
        }
    };

    let screen_right = origin_x.saturating_add(width as i32);
    let screen_bottom = origin_y.saturating_add(height as i32);
    elements
        .into_iter()
        .filter_map(|element| {
            let name = element.get_name().ok()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            let rect = element.get_bounding_rectangle().ok()?;
            let left = rect.get_left();
            let top = rect.get_top();
            let right = rect.get_right();
            let bottom = rect.get_bottom();
            if right <= left
                || bottom <= top
                || right <= origin_x
                || bottom <= origin_y
                || left >= screen_right
                || top >= screen_bottom
            {
                return None;
            }
            Some(GroundingCandidate {
                source: CandidateSource::UiAutomation,
                label: name,
                control_type: element.get_control_type().ok().map(|value| format!("{value:?}")),
                left,
                top,
                right,
                bottom,
                confidence: Some(1.0),
            })
        })
        .take(MAX_UIA_CANDIDATES)
        .collect()
}

fn collect_ocr_candidates(
    rgba: &[u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
) -> Vec<GroundingCandidate> {
    let Some(image) = RgbaImage::from_raw(width, height, rgba.to_vec()) else {
        return Vec::new();
    };
    let source = DynamicImage::ImageRgba8(image);
    let reduced = if width.max(height) > 1600 {
        source.resize(1600, 1600, image::imageops::FilterType::Triangle)
    } else {
        source
    };
    let rgb = reduced.to_rgb8();
    let mut ocr = Ocr::new(OcrOptions::new(BackendPreference::Native));
    let result = match ocr.recognize(&rgb) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[aura] OCR failed: {error}");
            return Vec::new();
        }
    };

    result
        .lines
        .into_iter()
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| {
            let left = origin_x + (line.rect.x.clamp(0.0, 1.0) * width as f32).round() as i32;
            let top = origin_y + (line.rect.y.clamp(0.0, 1.0) * height as f32).round() as i32;
            let right = origin_x
                + ((line.rect.x + line.rect.width).clamp(0.0, 1.0) * width as f32).round()
                    as i32;
            let bottom = origin_y
                + ((line.rect.y + line.rect.height).clamp(0.0, 1.0) * height as f32).round()
                    as i32;
            GroundingCandidate {
                source: CandidateSource::Ocr,
                label: line.text.trim().to_owned(),
                control_type: Some("text".to_owned()),
                left,
                top,
                right,
                bottom,
                confidence: line.confidence,
            }
        })
        .take(MAX_OCR_CANDIDATES)
        .collect()
}

pub fn token_overlap_score(query: &str, candidate: &str) -> f32 {
    let tokens = |value: &str| {
        value
            .to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| token.len() > 1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let query_tokens = tokens(query);
    let candidate_tokens = tokens(candidate);
    if query_tokens.is_empty() || candidate_tokens.is_empty() {
        return 0.0;
    }
    let matches = query_tokens
        .iter()
        .filter(|token| candidate_tokens.iter().any(|candidate| candidate == *token))
        .count();
    matches as f32 / query_tokens.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_prefers_semantic_match() {
        assert!(token_overlap_score("Save report button", "Save report") > 0.6);
        assert_eq!(token_overlap_score("Save report", "Cancel"), 0.0);
    }
}

/// Timing helpers for the live diagnostics in `agent.rs` (return only the candidate counts).
#[cfg(test)]
pub fn collect_uia_for_test(process_id: u32, origin_x: i32, origin_y: i32, width: u32, height: u32) -> usize {
    collect_uia_candidates(process_id, 0, origin_x, origin_y, width, height).len()
}

#[cfg(test)]
pub fn collect_ocr_for_test(rgba: &[u8], width: u32, height: u32, origin_x: i32, origin_y: i32) -> usize {
    collect_ocr_candidates(rgba, width, height, origin_x, origin_y).len()
}
