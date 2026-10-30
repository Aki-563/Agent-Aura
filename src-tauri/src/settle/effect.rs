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
    /// Longer ceiling while the screen shows a loading signal (spinner, progress bar, busy
    /// cursor, a hung window, the app busy on the CPU): acting on a half-loaded screen is worse
    /// than waiting. 0 = never extend.
    pub loading_max_ms: u64,
}

/// Default ceiling while something is visibly loading.
pub const LOADING_MAX_MS: u64 = 15_000;

impl SettleProfile {
    /// The same profile with its waiting times stretched for a slow or busy computer (factor 1..4).
    /// The stability window stays short: it measures quiet, not lag.
    pub fn scaled(self, factor: f32) -> Self {
        let f = factor.clamp(1.0, 4.0) as f64;
        let stretch = |ms: u64| (ms as f64 * f).round() as u64;
        Self {
            change_timeout_ms: stretch(self.change_timeout_ms),
            max_ms: stretch(self.max_ms),
            loading_max_ms: if self.loading_max_ms == 0 { 0 } else { stretch(self.loading_max_ms).max(stretch(self.max_ms)) },
            ..self
        }
    }
}

// ---- Adaptive timing: how slow is this computer right now? -----------------------------

static LAG_FACTOR_MILLI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1000);
static FIRST_CHANGE_EMA_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Current waiting-time multiplier (1.0 on an idle machine, up to 4.0 when it stutters).
pub fn lag_factor() -> f32 {
    LAG_FACTOR_MILLI.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
}

/// Pure rule: the factor from total CPU use (0..1) and the smoothed time an action took to show
/// its first visible change (ms).
pub fn lag_factor_from(cpu_busy: f32, first_change_ema_ms: u32) -> f32 {
    let cpu: f32 = if cpu_busy > 0.9 { 2.5 } else if cpu_busy > 0.75 { 1.6 } else { 1.0 };
    // A click normally shows its first change within ~250 ms; slower than that means lag.
    let latency = (first_change_ema_ms as f32 / 250.0).clamp(1.0, 3.0);
    cpu.max(latency).clamp(1.0, 4.0)
}

/// Total CPU use since the previous call (0..1), from `GetSystemTimes`.
fn cpu_busy_since_last_call() -> f32 {
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    static LAST: Mutex<Option<(u64, u64, u64)>> = Mutex::new(None);
    let as_u64 = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    let (mut idle, mut kernel, mut user) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return 0.0;
    }
    let now = (as_u64(idle), as_u64(kernel), as_u64(user));
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let busy = match *last {
        Some((i0, k0, u0)) => {
            let (di, dk, du) = (now.0.saturating_sub(i0), now.1.saturating_sub(k0), now.2.saturating_sub(u0));
            let total = dk + du; // kernel time includes idle time
            if total == 0 { 0.0 } else { 1.0 - di as f32 / total as f32 }
        }
        None => 0.0,
    };
    *last = Some(now);
    busy.clamp(0.0, 1.0)
}

/// Updates the lag factor after a settle (called by `wait_for_effect`).
fn update_lag(first_change_ms: Option<u64>) {
    use std::sync::atomic::Ordering::Relaxed;
    if let Some(ms) = first_change_ms {
        let previous = FIRST_CHANGE_EMA_MS.load(Relaxed);
        let ema = if previous == 0 { ms as u32 } else { (previous as f32 * 0.8 + ms as f32 * 0.2) as u32 };
        FIRST_CHANGE_EMA_MS.store(ema, Relaxed);
    }
    let factor = lag_factor_from(cpu_busy_since_last_call(), FIRST_CHANGE_EMA_MS.load(Relaxed));
    LAG_FACTOR_MILLI.store((factor * 1000.0) as u32, Relaxed);
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
        // Actions that can start a load (a click, Enter, opening a page) may wait for it to finish.
        let loading_max_ms = if max_ms == 0 || matches!(action, ActionPrimitive::WaitSettle { .. }) { 0 } else { LOADING_MAX_MS.max(max_ms) };
        Self { min_changed, change_timeout_ms, stable_ms, max_ms, loading_max_ms }.scaled(lag_factor())
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
    last_loading_ms: Option<u64>,
}

impl SettleMachine {
    pub fn new(profile: SettleProfile) -> Self {
        Self { profile, saw_change: false, last_motion_ms: 0, last_loading_ms: None }
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
        self.on_frame_ext(t_ms, changed_vs_baseline, changed_vs_previous, cursor_busy, false)
    }

    /// Like `on_frame`, with a loading signal (a spinner or progress bar keeps moving, the window
    /// is hung, the app is busy). While it lasts the wait may run past `max_ms`, up to
    /// `loading_max_ms`, so the next screenshot is not taken of a half-loaded screen.
    pub fn on_frame_ext(
        &mut self,
        t_ms: u64,
        changed_vs_baseline: u32,
        changed_vs_previous: u32,
        cursor_busy: bool,
        loading: bool,
    ) -> Option<SettleOutcome> {
        if changed_vs_baseline >= self.profile.min_changed {
            self.saw_change = true;
        }
        if changed_vs_previous >= self.profile.min_changed || cursor_busy || loading {
            self.last_motion_ms = t_ms;
        }
        if loading {
            self.last_loading_ms = Some(t_ms);
        }

        if self.saw_change && t_ms.saturating_sub(self.last_motion_ms) >= self.profile.stable_ms && !cursor_busy && !loading {
            return Some(SettleOutcome::Settled);
        }
        if !self.saw_change && !cursor_busy && !loading && t_ms >= self.profile.change_timeout_ms {
            return Some(SettleOutcome::NoEffect);
        }
        // Still loading recently: the ceiling is the loading ceiling, not the normal one.
        let loading_recently = self.last_loading_ms.is_some_and(|at| t_ms.saturating_sub(at) <= self.profile.stable_ms.max(300));
        let ceiling = if loading_recently && self.profile.loading_max_ms > 0 { self.profile.loading_max_ms.max(self.profile.max_ms) } else { self.profile.max_ms };
        if t_ms >= ceiling {
            return Some(if self.saw_change || loading_recently { SettleOutcome::Unstable } else { SettleOutcome::NoEffect });
        }
        None
    }

    /// Whether a loading signal was seen during this wait.
    pub fn saw_loading(&self) -> bool {
        self.last_loading_ms.is_some()
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
    /// The blinking text caret (frame pixels): never counted as motion.
    pub caret: Option<Roi>,
    /// The window stopped responding (`IsHungAppWindow`): it is busy, not finished.
    pub hung: bool,
    /// The app is busy on the CPU right now (a loading signal when the screen is not moving).
    pub app_busy: bool,
}

/// Counts changed samples like `count_changed_samples_in`, skipping `masks`, and returns the
/// bounding box of the changes (to tell a small spinner from a page that redraws).
pub fn count_changes_masked(a: &[u8], b: &[u8], width: u32, height: u32, roi: Option<Roi>, masks: &[Roi]) -> (u32, u32, Option<Roi>) {
    count_changes_masked_delta(a, b, width, height, roi, masks, PIXEL_DELTA)
}

/// Like `count_changes_masked` with an explicit luma threshold. A low one (0.025) sees the faint
/// gradient of a skeleton loader that the normal threshold ignores.
pub fn count_changes_masked_delta(a: &[u8], b: &[u8], width: u32, height: u32, roi: Option<Roi>, masks: &[Roi], delta: f32) -> (u32, u32, Option<Roi>) {
    let (w, h) = (width as usize, height as usize);
    if a.len() != b.len() || a.len() < w * h * 4 {
        return (u32::MAX, 1, None);
    }
    let (x0, y0, x1, y1) = match roi {
        Some(r) => (r.left.max(0) as usize, r.top.max(0) as usize, (r.right.max(0) as usize).min(w), (r.bottom.max(0) as usize).min(h)),
        None => (0, 0, w, h),
    };
    let luma = |frame: &[u8], o: usize| (0.299 * frame[o] as f32 + 0.587 * frame[o + 1] as f32 + 0.114 * frame[o + 2] as f32) / 255.0;
    let masked = |x: usize, y: usize| masks.iter().any(|m| (x as i32) >= m.left && (x as i32) < m.right && (y as i32) >= m.top && (y as i32) < m.bottom);
    let (mut changed, mut total) = (0u32, 0u32);
    let mut bbox: Option<Roi> = None;
    for y in (y0..y1).step_by(SAMPLE_STRIDE) {
        let row = y * w * 4;
        for x in (x0..x1).step_by(SAMPLE_STRIDE) {
            if masked(x, y) {
                continue;
            }
            total += 1;
            let o = row + x * 4;
            if (luma(a, o) - luma(b, o)).abs() > delta {
                changed += 1;
                let (xi, yi) = (x as i32, y as i32);
                bbox = Some(match bbox {
                    None => Roi { left: xi, top: yi, right: xi + 1, bottom: yi + 1 },
                    Some(r) => Roi { left: r.left.min(xi), top: r.top.min(yi), right: r.right.max(xi + 1), bottom: r.bottom.max(yi + 1) },
                });
            }
        }
    }
    (changed, total, bbox)
}

/// Motion confined to a small area of the window (a spinner, a progress bar, a shimmering
/// skeleton row) as opposed to a page that is redrawing: the classic "still loading" look.
pub fn is_small_persistent_motion(motion_box: Option<Roi>, window: Option<Roi>) -> bool {
    let (Some(m), Some(w)) = (motion_box, window) else { return false };
    let area = |r: Roi| ((r.right - r.left).max(0) as i64) * ((r.bottom - r.top).max(0) as i64);
    let window_area = area(w).max(1);
    // Up to a quarter of the window: spinners, bars and skeleton blocks; whole-page redraws are larger.
    // A text caret blinking (a thin bar a few pixels wide) is not a loading animation; a spinner,
    // progress bar or skeleton block is at least about a dozen pixels both ways.
    let (width, height) = ((m.right - m.left).max(0), (m.bottom - m.top).max(0));
    area(m) * 4 <= window_area && width >= 10 && height >= 10
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
    /// Why the wait took as long as it did ("a small animation kept moving", "busy cursor", ...).
    pub wait_reason: String,
    /// The screen still showed a loading signal when the wait ended (it hit the loading ceiling).
    pub still_loading: bool,
}

impl EffectReport {
    /// One line the model can act on in the next prompt.
    pub fn describe(&self, text_input: bool) -> String {
        match self.outcome {
            SettleOutcome::NoEffect if text_input => format!(
                "no visible change within {} ms; the typed text or key may not have registered (small text changes are hard to detect, so check the screenshot)",
                self.elapsed_ms
            ),
            SettleOutcome::Settled if !self.wait_reason.is_empty() => format!(
                "the screen changed ({:.1}% of it) and settled after {} ms (waited because {})",
                self.peak_changed_percent, self.elapsed_ms, self.wait_reason
            ),
            SettleOutcome::Settled => format!(
                "the screen changed ({:.1}% of it) and settled after {} ms",
                self.peak_changed_percent, self.elapsed_ms
            ),
            SettleOutcome::Unstable if self.still_loading => format!(
                "STILL LOADING after {:.1} s ({}): the page or app has not finished; wait (wait action) or check the screenshot before acting, and do not repeat the action",
                self.elapsed_ms as f64 / 1000.0,
                if self.wait_reason.is_empty() { "a loading animation kept moving" } else { &self.wait_reason }
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
/// (0 if unknown) and `focus_probe` reports the active window, its bounds, caret and busy state.
///
/// Loading signals (a small animation that keeps moving, the busy cursor, a hung window, the app
/// busy on the CPU) extend the wait past `max_ms` up to `loading_max_ms`, so the next screenshot
/// is not of a half-loaded screen. The blinking caret is masked out. The wait reason is reported.
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
            wait_reason: String::new(),
            still_loading: false,
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
    let mut first_change_ms: Option<u64> = None;
    let mut small_motion_frames = 0u32;
    let mut last_spinner_ms: Option<u64> = None;
    let mut faint_reference: (Vec<u8>, u64) = (baseline.to_vec(), 0);
    let mut reasons: Vec<&'static str> = Vec::new();
    let note = |reason: &'static str, reasons: &mut Vec<&'static str>| {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    };
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
            let masks: Vec<Roi> = focus.and_then(|f| f.caret).map(|c| Roi { left: c.left - 2, top: c.top - 2, right: c.right + 2, bottom: c.bottom + 2 }).into_iter().collect();
            let (mut vs_baseline, total, _) = count_changes_masked(baseline, &frame.data, width, height, roi, &masks);
            let (vs_previous, _, _motion_box) = count_changes_masked(&previous, &frame.data, width, height, roi, &masks);
            // A different active window than before the action is a visible effect by itself.
            if baseline_window != 0 && focus.is_some_and(|f| f.window_id != baseline_window) {
                vs_baseline = vs_baseline.max(profile.min_changed);
            }
            if first_change_ms.is_none() && vs_baseline >= profile.min_changed {
                first_change_ms = Some(t_ms);
            }
            frames += 1;
            let busy = CursorMonitor::is_system_busy();
            cursor_was_busy |= busy;
            if busy {
                note("the busy cursor was showing", &mut reasons);
            }
            // A small area that keeps moving frame after frame: a spinner, progress bar or skeleton.
            // Judged with a more sensitive threshold: a skeleton shimmer changes luma by under 0.07.
            // Against a reference frame up to 200 ms old, so slow drifts add up to something visible.
            let (faint, _, faint_box) = count_changes_masked_delta(&faint_reference.0, &frame.data, width, height, roi, &masks, 0.02);
            if t_ms.saturating_sub(faint_reference.1) >= 200 {
                faint_reference = (frame.data.clone(), t_ms);
            }
            if faint >= profile.min_changed && is_small_persistent_motion(faint_box, roi) {
                small_motion_frames += 1;
            } else if faint < profile.min_changed {
                small_motion_frames = 0;
            }
            if small_motion_frames >= 4 {
                last_spinner_ms = Some(t_ms);
            }
            // Held for 800 ms after the last loading motion, so the gap between two shimmer
            // sweeps (or a spinner's slow phase) does not count as "finished".
            let spinner = last_spinner_ms.is_some_and(|at| t_ms.saturating_sub(at) < 800);
            if spinner {
                note("a loading animation (spinner or progress bar) kept moving", &mut reasons);
            }
            let hung = focus.is_some_and(|f| f.hung);
            if hung {
                note("the window was not responding", &mut reasons);
            }
            let app_busy = focus.is_some_and(|f| f.app_busy) && machine.saw_change();
            if app_busy {
                note("the app was busy on the CPU", &mut reasons);
            }
            let loading = spinner || hung || app_busy;
            peak_percent = peak_percent.max(vs_baseline.min(total) as f32 * 100.0 / total.max(1) as f32);
            if let Some(outcome) = machine.on_frame_ext(t_ms, vs_baseline, vs_previous, busy, loading) {
                update_lag(first_change_ms);
                let waited_long = t_ms > profile.stable_ms + 1_500 || machine.saw_loading();
                return Ok(EffectReport {
                    outcome,
                    elapsed_ms: t_ms,
                    peak_changed_percent: peak_percent,
                    frames_compared: frames,
                    cursor_was_busy,
                    wait_reason: if waited_long { reasons.join(", ") } else { String::new() },
                    still_loading: outcome == SettleOutcome::Unstable && loading,
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
        SettleProfile { min_changed: MIN_CHANGED_SAMPLES, change_timeout_ms: 700, stable_ms: 250, max_ms: 4_000, loading_max_ms: 0 };
    const BIG: u32 = 500;
    const NONE: u32 = 0;

    const LOADING_PROFILE: SettleProfile =
        SettleProfile { min_changed: MIN_CHANGED_SAMPLES, change_timeout_ms: 700, stable_ms: 250, max_ms: 4_000, loading_max_ms: 15_000 };

    /// Frames every 40 ms: (until_ms, changed vs baseline, changed vs previous, loading).
    fn run_loading(phases: &[(u64, u32, u32, bool)]) -> (Option<SettleOutcome>, u64) {
        let mut machine = SettleMachine::new(LOADING_PROFILE);
        let mut t = 40;
        for &(until, base, prev, loading) in phases {
            while t <= until {
                if let Some(outcome) = machine.on_frame_ext(t, base, prev, false, loading) {
                    return (Some(outcome), t);
                }
                t += 40;
            }
        }
        (None, t)
    }

    #[test]
    fn a_spinner_keeps_the_wait_going_until_the_content_arrives() {
        // Click -> spinner for 9 s (past the 4 s ceiling) -> content appears and stays still.
        let (outcome, at) = run_loading(&[(9_000, BIG, 40, true), (12_000, BIG, NONE, false)]);
        assert_eq!(outcome, Some(SettleOutcome::Settled));
        assert!(at > 9_000 && at < 9_600, "settled {at} ms after the click, right after the load");
    }

    #[test]
    fn an_endless_spinner_stops_at_the_loading_ceiling() {
        let (outcome, at) = run_loading(&[(40_000, BIG, 40, true)]);
        assert_eq!(outcome, Some(SettleOutcome::Unstable));
        assert!((15_000..15_100).contains(&at), "gave up at {at} ms");
    }

    #[test]
    fn without_a_loading_signal_the_normal_ceiling_still_applies() {
        let (outcome, at) = run_loading(&[(40_000, BIG, BIG, false)]);
        assert_eq!(outcome, Some(SettleOutcome::Unstable));
        assert!((4_000..4_100).contains(&at));
        // And a click that changed nothing is still "no effect" quickly, spinner or not elsewhere.
        let (outcome, at) = run_loading(&[(5_000, NONE, NONE, false)]);
        assert_eq!(outcome, Some(SettleOutcome::NoEffect));
        assert!(at <= 760);
    }

    #[test]
    fn small_local_motion_is_loading_but_a_page_redraw_is_not() {
        let window = Some(Roi { left: 0, top: 0, right: 1000, bottom: 800 });
        assert!(is_small_persistent_motion(Some(Roi { left: 480, top: 380, right: 520, bottom: 420 }), window), "a 40 px spinner");
        assert!(is_small_persistent_motion(Some(Roi { left: 100, top: 500, right: 900, bottom: 520 }), window), "a progress bar");
        assert!(!is_small_persistent_motion(Some(Roi { left: 0, top: 0, right: 1000, bottom: 700 }), window), "a whole page redraw");
        assert!(!is_small_persistent_motion(None, window));
        assert!(!is_small_persistent_motion(Some(Roi { left: 300, top: 300, right: 303, bottom: 320 }), window), "a blinking text caret");
    }

    #[test]
    fn the_caret_is_masked_out_of_the_change_count() {
        let (w, h) = (100u32, 60u32);
        let a = vec![255u8; (w * h * 4) as usize];
        let mut b = a.clone();
        for y in 10..30 {
            for x in 50..52 {
                let o = ((y * w + x) * 4) as usize;
                b[o..o + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let (changed, _, bbox) = count_changes_masked(&a, &b, w, h, None, &[]);
        assert!(changed > 0 && bbox.is_some());
        let caret = Roi { left: 49, top: 9, right: 53, bottom: 31 };
        let (masked, _, none) = count_changes_masked(&a, &b, w, h, None, &[caret]);
        assert_eq!((masked, none), (0, None), "a blinking caret is not a change");
    }

    #[test]
    fn a_busy_or_slow_computer_stretches_the_waits() {
        assert_eq!(lag_factor_from(0.2, 150), 1.0);
        assert_eq!(lag_factor_from(0.95, 150), 2.5);
        assert!((lag_factor_from(0.2, 600) - 2.4).abs() < 0.01);
        assert_eq!(lag_factor_from(0.95, 5_000), 3.0_f32.max(2.5));
        let scaled = LOADING_PROFILE.scaled(2.0);
        assert_eq!((scaled.change_timeout_ms, scaled.max_ms, scaled.loading_max_ms, scaled.stable_ms), (1_400, 8_000, 30_000, 250));
    }

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
