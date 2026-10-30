//! Keeps raw x/y points from missing: snaps near misses onto the nearest numbered element and
//! learns each model's consistent offset or scale error (some models are always a little left or
//! low, or answer in a 0-1000 grid) so the same error can be removed from later steps.
//!
//! Points that came from an element number are never touched: they already are an element centre.
//! Nothing here depends on which model it is; the correction is learned from that model's own
//! answers, keyed by the model name only so that different models do not mix their samples.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::marks::Element;
use crate::input::send_input::ActionPrimitive;

/// A point this close (screenshot pixels) outside an element is moved onto it.
const SNAP_GAP: f64 = 14.0;
/// A miss this close to an element is taken as evidence of that model's pointing error.
const LEARN_GAP: f64 = 70.0;
const MIN_SAMPLES: usize = 6;
const MAX_SAMPLES: usize = 60;

type Sample = [f64; 4]; // raw x, raw y, wanted x, wanted y

static SAMPLES: Mutex<Option<HashMap<String, Vec<Sample>>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub scale_x: f64,
    pub offset_x: f64,
    pub scale_y: f64,
    pub offset_y: f64,
}

impl Fit {
    pub fn apply(&self, x: i32, y: i32) -> (i32, i32) {
        ((self.scale_x * x as f64 + self.offset_x).round() as i32, (self.scale_y * y as f64 + self.offset_y).round() as i32)
    }
}

fn is_centre(elements: &[Element], x: i32, y: i32) -> bool {
    elements.iter().any(|e| e.center() == (x, y))
}

/// Distance from the point to the element's box (0 when inside).
fn gap_to(element: &Element, x: i32, y: i32) -> f64 {
    let dx = (element.left - x).max(0).max(x - element.right) as f64;
    let dy = (element.top - y).max(0).max(y - element.bottom) as f64;
    dx.hypot(dy)
}

fn nearest<'a>(elements: &'a [Element], x: i32, y: i32) -> Option<(&'a Element, f64)> {
    elements
        .iter()
        .map(|e| (e, gap_to(e, x, y)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
}

/// Moves a point that just missed an element onto it (a click on an element's edge or a few pixels
/// outside it); points inside any element, or far from all of them, stay as they are.
pub fn snap(elements: &[Element], x: i32, y: i32) -> (i32, i32) {
    match nearest(elements, x, y) {
        Some((element, gap)) if gap > 0.0 && gap <= SNAP_GAP => {
            let inset = |low: i32, high: i32, value: i32| {
                let margin = ((high - low) / 4).clamp(1, 3);
                value.clamp(low + margin.min((high - low) / 2), high - margin.min((high - low) / 2))
            };
            (inset(element.left, element.right, x), inset(element.top, element.bottom, y))
        }
        _ => (x, y),
    }
}

/// Least-squares fit of `wanted = scale * raw + offset` for one axis, or `None`.
fn fit_axis(raw: &[f64], wanted: &[f64]) -> Option<(f64, f64)> {
    let n = raw.len() as f64;
    let mean_raw = raw.iter().sum::<f64>() / n;
    let mean_wanted = wanted.iter().sum::<f64>() / n;
    let spread: f64 = raw.iter().map(|r| (r - mean_raw).powi(2)).sum();
    let range = raw.iter().cloned().fold(f64::MIN, f64::max) - raw.iter().cloned().fold(f64::MAX, f64::min);
    // Too little spread to tell a scale error from an offset: assume an offset only.
    if range < 300.0 || spread <= 0.0 {
        return Some((1.0, mean_wanted - mean_raw));
    }
    let covariance: f64 = raw.iter().zip(wanted).map(|(r, w)| (r - mean_raw) * (w - mean_wanted)).sum();
    let scale = covariance / spread;
    if !(0.8..=1.25).contains(&scale) {
        return Some((1.0, mean_wanted - mean_raw));
    }
    Some((scale, mean_wanted - scale * mean_raw))
}

fn fit_all(samples: &[&Sample]) -> Option<Fit> {
    let column = |index: usize| samples.iter().map(|s| s[index]).collect::<Vec<f64>>();
    let (scale_x, offset_x) = fit_axis(&column(0), &column(2))?;
    let (scale_y, offset_y) = fit_axis(&column(1), &column(3))?;
    Some(Fit { scale_x, offset_x, scale_y, offset_y })
}

fn miss(fit: &Fit, sample: &Sample) -> f64 {
    let (x, y) = fit.apply(sample[0] as i32, sample[1] as i32);
    (sample[2] - x as f64).hypot(sample[3] - y as f64)
}

/// The correction supported by the samples, only when the error is consistent and worth removing.
pub fn fit(samples: &[Sample]) -> Option<Fit> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let all: Vec<&Sample> = samples.iter().collect();
    let first = fit_all(&all)?;
    // Drop samples the first fit explains badly (the model aimed at something else).
    let kept: Vec<&Sample> = all.iter().copied().filter(|s| miss(&first, s) <= 25.0).collect();
    if kept.len() < MIN_SAMPLES {
        return None;
    }
    let fit = fit_all(&kept)?;
    let error_before = kept.iter().map(|s| (s[2] - s[0]).hypot(s[3] - s[1])).sum::<f64>() / kept.len() as f64;
    let error_after = kept.iter().map(|s| miss(&fit, s)).sum::<f64>() / kept.len() as f64;
    (error_before >= 6.0 && error_after <= error_before * 0.6).then_some(fit)
}

fn with_samples<R>(model: &str, f: impl FnOnce(&mut Vec<Sample>) -> R) -> R {
    let mut guard = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(load);
    f(map.entry(model.to_owned()).or_default())
}

fn store_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join("AgentAura").join("coord_calibration.json"))
}

fn load() -> HashMap<String, Vec<Sample>> {
    store_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(map: &HashMap<String, Vec<Sample>>) {
    if let Some(path) = store_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(map) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Notes a miss near an element as evidence of this model's pointing error.
fn learn(model: &str, elements: &[Element], x: i32, y: i32) {
    let Some((element, gap)) = nearest(elements, x, y) else { return };
    if gap <= 0.0 || gap > LEARN_GAP {
        return;
    }
    let (wanted_x, wanted_y) = element.center();
    with_samples(model, |samples| {
        samples.push([x as f64, y as f64, wanted_x as f64, wanted_y as f64]);
        if samples.len() > MAX_SAMPLES {
            samples.remove(0);
        }
    });
    let guard = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = guard.as_ref() {
        save(map);
    }
}

fn correction_for(model: &str) -> Option<Fit> {
    with_samples(model, |samples| fit(samples))
}

/// Corrects one raw point: applies the learned correction, then snaps a near miss onto an element.
/// A point that is an element centre (it came from an element number) is returned untouched.
pub fn refine_point(model: &str, elements: &[Element], size: (u32, u32), x: i32, y: i32) -> (i32, i32) {
    if elements.is_empty() || is_centre(elements, x, y) {
        return (x, y);
    }
    learn(model, elements, x, y);
    let (mut px, mut py) = (x, y);
    if let Some(fit) = correction_for(model) {
        let (cx, cy) = fit.apply(x, y);
        // Only trust a correction that keeps the point on the screenshot.
        if cx >= 0 && cy >= 0 && cx < size.0 as i32 && cy < size.1 as i32 {
            (px, py) = (cx, cy);
        }
    }
    snap(elements, px, py)
}

/// How sure grounding mode C is that the point is on what the model described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFit {
    /// The point is inside an element whose text matches the description.
    Confirmed,
    /// The point was moved onto the nearest element whose text matches.
    Moved,
    /// Nothing matched near the point; the crosshair verifier should look at it.
    Unconfirmed,
}

/// A match radius (screenshot pixels) within which a differently placed matching element is
/// preferred over where the model pointed.
const TARGET_RADIUS: f64 = 60.0;

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase).collect()
}

/// Whether the element's label is the thing the model described (`wanted` = visible text, else the
/// description). Every word of the label must appear in `wanted`, or the label contains `wanted`.
fn label_matches(label: &str, wanted: &[String], wanted_text: &str) -> bool {
    let label_words = words(label);
    if label_words.is_empty() || wanted.is_empty() {
        return false;
    }
    let lowered = label.to_lowercase();
    lowered.contains(&wanted_text.to_lowercase()) || label_words.iter().all(|w| wanted.contains(w))
}

/// Grounding mode C, no extra model call: hovers (in effect) over the point and checks that the
/// element under it is the one described. If another element near the point matches the
/// description instead, the point moves there.
pub fn refine_to_target(elements: &[Element], point: (i32, i32), target_text: &str) -> ((i32, i32), TargetFit) {
    let wanted = words(target_text);
    let mut matching: Vec<&Element> = elements.iter().filter(|e| label_matches(&e.label, &wanted, target_text)).collect();
    // An exact label ("Save" among "Save" and "Save As") beats a partial one.
    let exact: Vec<&Element> = matching.iter().copied().filter(|e| e.label.trim().eq_ignore_ascii_case(target_text.trim())).collect();
    if !exact.is_empty() {
        matching = exact;
    }
    if matching.is_empty() {
        return (point, TargetFit::Unconfirmed);
    }
    if matching.iter().any(|e| gap_to(e, point.0, point.1) == 0.0) {
        return (point, TargetFit::Confirmed);
    }
    let best = matching.iter().map(|e| (*e, gap_to(e, point.0, point.1))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    match best {
        Some((element, gap)) if gap <= TARGET_RADIUS => (element.center(), TargetFit::Moved),
        _ => (point, TargetFit::Unconfirmed),
    }
}

/// Applies `refine_to_target` to the first raw point of a pointer action. Returns the action and the
/// fit (`None` for actions without a point).
pub fn refine_action_to_target(elements: &[Element], target_text: &str, action: ActionPrimitive) -> (ActionPrimitive, Option<TargetFit>) {
    let mut action = action;
    let mut fit = None;
    let mut fix = |x: &mut i32, y: &mut i32| {
        let ((nx, ny), f) = refine_to_target(elements, (*x, *y), target_text);
        *x = nx;
        *y = ny;
        fit = Some(f);
    };
    match &mut action {
        ActionPrimitive::Click { x, y, .. } | ActionPrimitive::DoubleClick { x, y, .. } | ActionPrimitive::MouseMove { x, y } => fix(x, y),
        ActionPrimitive::DragAndDrop { from_x, from_y, .. } => fix(from_x, from_y),
        _ => {}
    }
    (action, fit)
}

/// Applies `refine_point` to every raw point in a pointer action.
pub fn refine_action(model: &str, elements: &[Element], size: (u32, u32), action: ActionPrimitive) -> ActionPrimitive {
    let mut action = action;
    let fix = |x: &mut i32, y: &mut i32| {
        let (nx, ny) = refine_point(model, elements, size, *x, *y);
        *x = nx;
        *y = ny;
    };
    match &mut action {
        ActionPrimitive::Click { x, y, .. } | ActionPrimitive::DoubleClick { x, y, .. } | ActionPrimitive::MouseMove { x, y } => fix(x, y),
        ActionPrimitive::DragAndDrop { from_x, from_y, to_x, to_y, .. } => {
            fix(from_x, from_y);
            fix(to_x, to_y);
        }
        _ => {}
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(id: u32, left: i32, top: i32, right: i32, bottom: i32) -> Element {
        Element { id, label: format!("e{id}"), kind: "Button".into(), left, top, right, bottom }
    }

    #[test]
    fn mode_c_recovers_near_misses_onto_the_described_element() {
        // A row of small buttons 8 px apart; the model "meant" one but missed by 1..20 px in a random direction.
        let names = ["Save", "Save As", "Open", "Close", "Print", "Share", "Export", "Import"];
        let elements: Vec<Element> = names.iter().enumerate().map(|(i, n)| {
            let left = 40 + i as i32 * 78;
            Element { id: i as u32 + 1, label: (*n).into(), kind: "Button".into(), left, top: 100, right: left + 70, bottom: 124 }
        }).collect();
        let mut seed = 12345u64;
        let mut next = |m: i32| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((seed >> 33) as i32).rem_euclid(m) };
        let (mut recovered, mut total) = (0, 0);
        for _ in 0..400 {
            let target = &elements[next(elements.len() as i32) as usize];
            let (cx, cy) = target.center();
            let (dx, dy) = (next(41) - 20, next(41) - 20);
            let (point, _) = refine_to_target(&elements, (cx + dx, cy + dy), &target.label);
            total += 1;
            let inside = point.0 >= target.left && point.0 <= target.right && point.1 >= target.top && point.1 <= target.bottom;
            if ((point.0 - cx) as f64).hypot((point.1 - cy) as f64) <= 14.0 || inside {
                recovered += 1;
            }
        }
        assert!(recovered * 100 >= total * 95, "recovered {recovered} of {total}");
        // Nothing matching the description nearby -> unconfirmed (the crosshair verifier takes over).
        assert_eq!(refine_to_target(&elements, (300, 110), "Settings gear").1, TargetFit::Unconfirmed);
        assert_eq!(refine_to_target(&elements, (75, 112), "Save").1, TargetFit::Confirmed);
    }

    #[test]
    fn a_near_miss_is_pulled_onto_the_element() {
        let elements = [element(1, 100, 100, 200, 140)];
        assert_eq!(snap(&elements, 150, 120), (150, 120), "inside stays");
        let (x, y) = snap(&elements, 207, 120);
        assert!((197..=200).contains(&x) && y == 120, "got {x},{y}");
        assert_eq!(snap(&elements, 400, 400), (400, 400), "far away stays");
    }

    #[test]
    fn a_consistent_offset_is_learned_and_removed() {
        // The model always lands 30 px right and 20 px low of what it means.
        let samples: Vec<Sample> = (0..8)
            .map(|i| {
                let (wx, wy) = (100.0 + 80.0 * i as f64, 150.0 + 40.0 * i as f64);
                [wx + 30.0, wy + 20.0, wx, wy]
            })
            .collect();
        let fit = fit(&samples).expect("a clear offset must be learned");
        let (x, y) = fit.apply(330, 220);
        assert!((x - 300).abs() <= 2 && (y - 200).abs() <= 2, "got {x},{y}");
    }

    #[test]
    fn a_scale_error_is_learned() {
        // The model's points are 10% too small: raw = wanted * 0.9. Wilder scale errors are left
        // alone rather than guessed.
        let samples: Vec<Sample> = (0..10)
            .map(|i| {
                let (wx, wy) = (60.0 + 120.0 * i as f64, 50.0 + 60.0 * i as f64);
                [wx * 0.9, wy * 0.9, wx, wy]
            })
            .collect();
        let fit = fit(&samples).expect("a clear scale error must be learned");
        let (x, y) = fit.apply(900, 450);
        assert!((x - 1000).abs() <= 3 && (y - 500).abs() <= 3, "got {x},{y}");
    }

    #[test]
    fn scattered_or_too_few_samples_learn_nothing() {
        assert!(fit(&[[10.0, 10.0, 40.0, 40.0]; 3]).is_none());
        let noisy: Vec<Sample> = (0..8)
            .map(|i| {
                let jitter = if i % 2 == 0 { 60.0 } else { -60.0 };
                [100.0 + 50.0 * i as f64, 100.0, 100.0 + 50.0 * i as f64 + jitter, 100.0]
            })
            .collect();
        assert!(fit(&noisy).is_none());
    }

    #[test]
    fn element_centres_are_never_touched() {
        let elements = [element(1, 100, 100, 200, 140)];
        let centre = elements[0].center();
        assert_eq!(refine_point("unit-test-model", &elements, (1280, 720), centre.0, centre.1), centre);
    }
}
