//! Change-then-stable settle detection (replaces the old "diff against a frame taken after
//! the action" check, which could not tell "nothing happened yet" from "finished").
//!
//! The baseline is the frame the model *saw*, captured before the input was sent:
//!
//! ```text
//! WAIT_FOR_CHANGE --(screen differs from baseline)--> WAIT_FOR_STABLE --(quiet for stable_ms)--> Settled
//!        | change_timeout elapsed, nothing changed                          | max_ms elapsed
//!        v                                                                  v
//!    NoEffect (reported, not an error)                                   Unstable (proceed)
//! ```

use std::thread;
use std::time::{Duration, Instant};

use crate::capture::dxgi::DxgiCapture;
use crate::input::ActionPrimitive;
use crate::settle::cursor::CursorMonitor;

/// Sampled pixels whose luma moved by more than this count as changed (0..1).
const PIXEL_DELTA: f32 = 0.06;
/// Sample every Nth pixel in both directions.
const SAMPLE_STRIDE: usize = 2;
/// Fewer changed samples than this is noise (a blinking caret is about 8 samples at stride 2;
/// a typed character is typically 20+).
pub const MIN_CHANGED_SAMPLES: u32 = 12;
/// Lower bar for typing and key presses (a 19-character command moved about 29 samples).
pub const TEXT_MIN_CHANGED_SAMPLES: u32 = 3;

/// Actions whose effect is small text changes rather than visible UI changes.
pub fn is_text_input(action: &ActionPrimitive) -> bool {
    matches!(action, ActionPrimitive::TypeText { .. } | ActionPrimitive::KeyPress { .. })
}
/// Interval between frame comparisons.
const POLL_INTERVAL_MS: u64 = 40;
/// Input needs a moment to reach the target application's message queue.
const INPUT_INGESTION_MS: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettleProfile {
    /// Changed samples needed to count as a visible change (typing needs a lower bar: one
    /// character moves only a handful of samples, which a caret blink also does).
    pub min_changed: u32,
    /// How long to wait for the first visible change before declaring "no effect".
    pub change_timeout_ms: u64,
    /// How long the screen must stay quiet after changing to count as settled.
    pub stable_ms: u64,
    /// Hard ceiling for the whole wait.
    pub max_ms: u64,
}

impl SettleProfile {
    pub fn for_action(action: &ActionPrimitive) -> Self {
        let (change_timeout_ms, stable_ms, max_ms) = match action {
            ActionPrimitive::TypeText { .. } | ActionPrimitive::KeyPress { .. } => (300, 150, 1_500),
            ActionPrimitive::Scroll { .. } => (400, 200, 2_000),
            ActionPrimitive::Hotkey { .. } => (1_200, 300, 5_000),
            ActionPrimitive::OpenApp { .. } => (3_000, 500, 12_000),
            ActionPrimitive::FocusWindow { .. } => (1_200, 300, 4_000),
            ActionPrimitive::OpenUrl { .. } => (3_000, 500, 15_000),
            ActionPrimitive::Note { .. } | ActionPrimitive::AskUser { .. } => (0, 0, 0),
            ActionPrimitive::WaitSettle { max_timeout_ms, .. } => (0, 300, (*max_timeout_ms).clamp(300, 10_000)),
            ActionPrimitive::MouseMove { .. } => (300, 200, 1_500),
            ActionPrimitive::Click { .. }
            | ActionPrimitive::DoubleClick { .. }
            | ActionPrimitive::DragAndDrop { .. } => (700, 250, 4_000),
            ActionPrimitive::Done { .. } | ActionPrimitive::Fail { .. } => (0, 0, 0),
            ActionPrimitive::TypeWith { .. } | ActionPrimitive::KeyDown { .. } | ActionPrimitive::KeyUp { .. } | ActionPrimitive::HoldKey { .. } => (300, 150, 1_500),
            ActionPrimitive::ScrollFine { .. } => (400, 200, 2_000),
            ActionPrimitive::Hover { .. } => (300, 200, 1_500),
            ActionPrimitive::MouseDown { .. } | ActionPrimitive::MouseUp { .. } | ActionPrimitive::DrawPath { .. } | ActionPrimitive::Touch { .. } => (700, 250, 4_000),
            ActionPrimitive::WindowAction { .. } | ActionPrimitive::LayoutWindows { .. } | ActionPrimitive::SwitchDesktop { .. } => (1_200, 400, 5_000),
            // No input: nothing on screen is expected to change.
            ActionPrimitive::Zoom { .. } | ActionPrimitive::Observe {} | ActionPrimitive::ClipboardRead {} | ActionPrimitive::ClipboardWrite { .. } => (0, 0, 0),
        };
        let min_changed = if is_text_input(action) { TEXT_MIN_CHANGED_SAMPLES } else { MIN_CHANGED_SAMPLES };
        Self { min_changed, change_timeout_ms, stable_ms, max_ms }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettleOutcome {
    /// The screen changed and then stayed stable.
    Settled,
    /// Nothing visibly changed within the change timeout.
    NoEffect,
    /// The screen kept changing until the ceiling (animation, video, long load).
    Unstable,
}

/// Pure decision logic, driven by (time, changed-vs-baseline, changed-vs-previous, busy).
#[derive(Debug)]
pub struct SettleMachine {
    profile: SettleProfile,
    saw_change: bool,
    last_motion_ms: u64,
}

impl SettleMachine {
    pub fn new(profile: SettleProfile) -> Self {
        Self { profile, saw_change: false, last_motion_ms: 0 }
    }

    pub fn saw_change(&self) -> bool {
        self.saw_change
    }

    /// Feeds one observation. Returns the final outcome once it is decided.
    pub fn on_frame(
        &mut self,
        t_ms: u64,
        changed_vs_baseline: u32,
        changed_vs_previous: u32,
        cursor_busy: bool,
    ) -> Option<SettleOutcome> {
        if changed_vs_baseline >= self.profile.min_changed {
            self.saw_change = true;
        }
        if changed_vs_previous >= self.profile.min_changed || cursor_busy {
            self.last_motion_ms = t_ms;
        }

        if self.saw_change && t_ms.saturating_sub(self.last_motion_ms) >= self.profile.stable_ms && !cursor_busy {
            return Some(SettleOutcome::Settled);
        }
        if !self.saw_change && !cursor_busy && t_ms >= self.profile.change_timeout_ms {
            return Some(SettleOutcome::NoEffect);
        }
        if t_ms >= self.profile.max_ms {
            return Some(if self.saw_change { SettleOutcome::Unstable } else { SettleOutcome::NoEffect });
        }
        None
    }
}

/// A rectangle in frame pixels (right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roi {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Where the user's attention is: the active window and its bounds in frame pixels.
#[derive(Debug, Clone, Copy)]
pub struct Focus {
    /// Identifies the window (a window handle); a different id than the baseline means the
    /// active window changed, which is itself a visible effect.
    pub window_id: isize,
    pub roi: Roi,
}

/// Counts sampled pixels whose luma differs noticeably between two same-sized RGBA frames.
/// Returns (changed samples, total samples).
pub fn count_changed_samples(a: &[u8], b: &[u8], width: u32, height: u32) -> (u32, u32) {
    count_changed_samples_in(a, b, width, height, None)
}

/// Like `count_changed_samples`, but only inside `roi` (the whole frame when `None`).
/// Movement elsewhere on the screen (a video, a streaming chat, a clock) is ignored.
pub fn count_changed_samples_in(a: &[u8], b: &[u8], width: u32, height: u32, roi: Option<Roi>) -> (u32, u32) {
    let (width, height) = (width as usize, height as usize);
    if a.len() != b.len() || a.len() < width * height * 4 {
        return (u32::MAX, 1); // different geometry: treat as a full change
    }
    let (x0, y0, x1, y1) = match roi {
        Some(r) => (
            r.left.max(0) as usize,
            r.top.max(0) as usize,
            (r.right.max(0) as usize).min(width),
            (r.bottom.max(0) as usize).min(height),
        ),
        None => (0, 0, width, height),
    };
    let luma = |frame: &[u8], offset: usize| {
        (0.299 * frame[offset] as f32 + 0.587 * frame[offset + 1] as f32 + 0.114 * frame[offset + 2] as f32) / 255.0
    };
    let mut changed = 0u32;
    let mut total = 0u32;
    for y in (y0..y1).step_by(SAMPLE_STRIDE) {
        let row = y * width * 4;
        for x in (x0..x1).step_by(SAMPLE_STRIDE) {
            let offset = row + x * 4;
            total += 1;
            if (luma(a, offset) - luma(b, offset)).abs() > PIXEL_DELTA {
                changed += 1;
            }
        }
    }
    (changed, total)
}

/// Frames that are almost entirely black are what a fresh duplication can briefly return.
fn is_mostly_black(frame: &[u8]) -> bool {
    let mut bright = 0usize;
    let mut seen = 0usize;
    for pixel in frame.chunks_exact(4).step_by(997) {
        seen += 1;
        if pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32 > 24 {
            bright += 1;
        }
    }
    seen > 0 && bright * 100 / seen < 1
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReport {
    pub outcome: SettleOutcome,
    pub elapsed_ms: u64,
    /// Peak percentage of sampled pixels that differed from the pre-action frame.
    pub peak_changed_percent: f32,
    pub frames_compared: u32,
    pub cursor_was_busy: bool,
}

impl EffectReport {
    /// One line the model can act on in the next prompt.
    pub fn describe(&self, text_input: bool) -> String {
        match self.outcome {
            SettleOutcome::NoEffect if text_input => format!(
                "no visible change within {} ms; the typed text or key may not have registered (small text changes are hard to detect, so check the screenshot)",
                self.elapsed_ms
            ),
            SettleOutcome::Settled => format!(
                "the screen changed ({:.1}% of it) and settled after {} ms",
                self.peak_changed_percent, self.elapsed_ms
            ),
            SettleOutcome::NoEffect => format!(
                "NO VISIBLE EFFECT: the screen did not change within {} ms, so the action probably missed its target or was ignored",
                self.elapsed_ms
            ),
            SettleOutcome::Unstable => format!(
                "the screen was still changing after {} ms (animation or loading); it may not be finished",
                self.elapsed_ms
            ),
        }
    }
}

/// Waits for the screen to react to an action that was just sent.
///
/// `baseline` must be the full-resolution RGBA frame captured *before* the action, of the same
/// monitor that `capture` duplicates. `baseline_window` is the active window before the action
/// (0 if unknown) and `focus_probe` reports the active window and its bounds right now.
pub fn wait_for_effect(
    capture: &mut DxgiCapture,
    baseline: &[u8],
    profile: SettleProfile,
    baseline_window: isize,
    focus_probe: &dyn Fn() -> Option<Focus>,
) -> Result<EffectReport, String> {
    if profile.max_ms == 0 {
        return Ok(EffectReport {
            outcome: SettleOutcome::Settled,
            elapsed_ms: 0,
            peak_changed_percent: 0.0,
            frames_compared: 0,
            cursor_was_busy: false,
        });
    }
    let (width, height) = (capture.width(), capture.height());
    // A timeout then means "no change" rather than an error on a static screen.
    capture.seed_frame(baseline.to_vec());
    let started = Instant::now();
    let mut machine = SettleMachine::new(profile);
    let mut previous: Vec<u8> = baseline.to_vec();
    let mut peak_percent = 0.0f32;
    let mut frames = 0u32;
    let mut cursor_was_busy = false;
    thread::sleep(Duration::from_millis(INPUT_INGESTION_MS));

    loop {
        let sample_started = Instant::now();
        // A timeout returns the previous frame, which compares as "no change".
        let frame = capture
            .capture_frame(POLL_INTERVAL_MS as u32)
            .map_err(|error| format!("capture during settle failed: {error}"))?;
        let t_ms = started.elapsed().as_millis() as u64;

        if !is_mostly_black(&frame.data) || is_mostly_black(baseline) {
            // Judge the window the user is working in; ignore unrelated movement elsewhere.
            let focus = focus_probe();
            let roi = focus.map(|f| f.roi);
            let (mut vs_baseline, total) = count_changed_samples_in(baseline, &frame.data, width, height, roi);
            let (vs_previous, _) = count_changed_samples_in(&previous, &frame.data, width, height, roi);
            // A different active window than before the action is a visible effect by itself.
            if baseline_window != 0 && focus.is_some_and(|f| f.window_id != baseline_window) {
                vs_baseline = vs_baseline.max(profile.min_changed);
            }
            frames += 1;
            let busy = CursorMonitor::is_system_busy();
            cursor_was_busy |= busy;
            peak_percent = peak_percent.max(vs_baseline.min(total) as f32 * 100.0 / total.max(1) as f32);
            if let Some(outcome) = machine.on_frame(t_ms, vs_baseline, vs_previous, busy) {
                return Ok(EffectReport {
                    outcome,
                    elapsed_ms: t_ms,
                    peak_changed_percent: peak_percent,
                    frames_compared: frames,
                    cursor_was_busy,
                });
            }
            previous = frame.data;
        } else if t_ms >= profile.max_ms {
            return Err("the display returned only black frames during settle".to_owned());
        }

        let spent = sample_started.elapsed().as_millis() as u64;
        if spent < POLL_INTERVAL_MS {
            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS - spent));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: SettleProfile =
        SettleProfile { min_changed: MIN_CHANGED_SAMPLES, change_timeout_ms: 700, stable_ms: 250, max_ms: 4_000 };
    const BIG: u32 = 500;
    const NONE: u32 = 0;

    fn run(frames: &[(u64, u32, u32, bool)]) -> Option<SettleOutcome> {
        let mut machine = SettleMachine::new(PROFILE);
        frames
            .iter()
            .find_map(|(t, base, prev, busy)| machine.on_frame(*t, *base, *prev, *busy))
    }

    #[test]
    fn no_change_reports_no_effect_after_the_change_timeout() {
        let frames: Vec<_> = (1..40).map(|i| (i * 40, NONE, NONE, false)).collect();
        assert_eq!(run(&frames), Some(SettleOutcome::NoEffect));
        // ...but not before the timeout.
        assert_eq!(run(&frames[..10]), None);
    }

    #[test]
    fn change_then_quiet_settles_after_stable_ms() {
        let frames = [
            (40, NONE, NONE, false),
            (80, BIG, BIG, false),   // menu opens
            (120, BIG, BIG, false),  // still animating
            (160, BIG, NONE, false), // quiet starts being counted from the last motion (120)
            (280, BIG, NONE, false),
            (400, BIG, NONE, false), // 400 - 120 >= 250
        ];
        assert_eq!(run(&frames), Some(SettleOutcome::Settled));
        assert_eq!(run(&frames[..5]), None, "must still be waiting at 280 ms");
    }

    #[test]
    fn caret_blink_noise_is_neither_a_change_nor_motion() {
        let noise = MIN_CHANGED_SAMPLES - 1;
        let frames: Vec<_> = (1..40).map(|i| (i * 40, noise, noise, false)).collect();
        assert_eq!(run(&frames), Some(SettleOutcome::NoEffect));
    }

    #[test]
    fn delayed_change_after_400ms_is_still_caught() {
        let mut frames: Vec<_> = (1..10).map(|i| (i * 40, NONE, NONE, false)).collect();
        frames.push((400, BIG, BIG, false));
        frames.extend((11..30).map(|i| (i * 40, BIG, NONE, false)));
        assert_eq!(run(&frames), Some(SettleOutcome::Settled));
    }

    #[test]
    fn endless_animation_ends_as_unstable_at_the_ceiling() {
        let frames: Vec<_> = (1..120).map(|i| (i * 40, BIG, BIG, false)).collect();
        assert_eq!(run(&frames), Some(SettleOutcome::Unstable));
    }

    #[test]
    fn busy_cursor_delays_settling_and_no_effect() {
        // A spinner cursor with an unchanged screen is not "no effect" yet.
        let busy: Vec<_> = (1..30).map(|i| (i * 40, NONE, NONE, true)).collect();
        assert_eq!(run(&busy), None);
        // Once the spinner stops and the screen changed, it settles after the quiet period.
        let mut frames: Vec<_> = (1..10).map(|i| (i * 40, BIG, NONE, true)).collect();
        frames.extend((10..30).map(|i| (i * 40, BIG, NONE, false)));
        assert_eq!(run(&frames), Some(SettleOutcome::Settled));
    }

    #[test]
    fn counts_small_changes_but_ignores_tiny_ones() {
        let (width, height) = (320u32, 200u32);
        let base = vec![255u8; (width * height * 4) as usize];
        let mut typed = base.clone();
        // A 12x14 px dark glyph, like a typed character.
        for y in 50..64 {
            for x in 100..112 {
                let o = ((y * width + x) * 4) as usize;
                typed[o] = 0;
                typed[o + 1] = 0;
                typed[o + 2] = 0;
            }
        }
        let (changed, total) = count_changed_samples(&base, &typed, width, height);
        assert!(changed >= MIN_CHANGED_SAMPLES, "a typed character must register, got {changed}");
        assert!(total > 10_000);
        let (same, _) = count_changed_samples(&base, &base, width, height);
        assert_eq!(same, 0);
    }

    #[test]
    fn typing_uses_a_more_sensitive_threshold_than_clicks() {
        let typing = SettleProfile::for_action(&ActionPrimitive::KeyPress { key: "a".into() });
        let click = SettleProfile::for_action(&ActionPrimitive::Click {
            x: 1,
            y: 1,
            button: Default::default(),
            clicks: 1,
            hold_keys: vec![],
        });
        assert_eq!(typing.min_changed, TEXT_MIN_CHANGED_SAMPLES);
        assert_eq!(click.min_changed, MIN_CHANGED_SAMPLES);
        // 5 changed samples: a typed character for typing, noise for a click.
        let mut typing_machine = SettleMachine::new(typing);
        assert!(typing_machine.on_frame(40, 5, 5, false).is_none() && typing_machine.saw_change());
        let mut click_machine = SettleMachine::new(click);
        click_machine.on_frame(40, 5, 5, false);
        assert!(!click_machine.saw_change());
    }

    #[test]
    fn movement_outside_the_active_window_is_ignored() {
        let (width, height) = (400u32, 200u32);
        let base = vec![255u8; (width * height * 4) as usize];
        let mut moved = base.clone();
        // A busy region in the right half (video / streaming text) ...
        for y in 20..180 {
            for x in 250..390 {
                let o = ((y * width + x) * 4) as usize;
                moved[o] = 0;
                moved[o + 1] = 0;
                moved[o + 2] = 0;
            }
        }
        let whole = count_changed_samples_in(&base, &moved, width, height, None).0;
        let left_window = Roi { left: 0, top: 0, right: 200, bottom: 200 };
        let focused = count_changed_samples_in(&base, &moved, width, height, Some(left_window)).0;
        assert!(whole > MIN_CHANGED_SAMPLES, "the busy region is visible on the whole screen");
        assert_eq!(focused, 0, "...but not inside the active window");
        let overlapping = Roi { left: 200, top: 0, right: 400, bottom: 200 };
        assert!(count_changed_samples_in(&base, &moved, width, height, Some(overlapping)).0 > MIN_CHANGED_SAMPLES);
        // A region outside the frame is clamped, never a panic.
        let huge = Roi { left: -50, top: -50, right: 9_000, bottom: 9_000 };
        assert_eq!(count_changed_samples_in(&base, &moved, width, height, Some(huge)).0, whole);
    }

    #[test]
    fn profiles_scale_with_the_action() {
        let type_text = SettleProfile::for_action(&ActionPrimitive::TypeText {
            text: "x".into(),
            press_enter: false,
            overwrite: false,
        });
        let hotkey = SettleProfile::for_action(&ActionPrimitive::Hotkey { keys: vec!["win".into()] });
        assert!(type_text.max_ms < hotkey.max_ms);
        assert_eq!(SettleProfile::for_action(&ActionPrimitive::Done { summary: String::new() }).max_ms, 0);
    }

    #[test]
    fn black_frames_are_recognised() {
        assert!(is_mostly_black(&vec![0u8; 4 * 10_000]));
        assert!(!is_mostly_black(&vec![200u8; 4 * 10_000]));
    }
}
