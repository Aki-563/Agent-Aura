//! High-performance animated neon screen-edge glow and glowing halo cursor (Phase 2 implementation)
//!
//! Renders real-time hardware-accelerated visual state cues (Purple Thinking,
//! Cyan Working, Amber Settling, Red Emergency) with sine-wave breathing pulses
//! and dynamic water-ripple click effects onto the transparent overlay.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, UpdateLayeredWindow, SM_CXSCREEN, ULW_ALPHA,
};

use crate::overlay::window::OverlayWindow;

/// Agent Aura visual HUD states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudState {
    /// Deep Electric Purple (#9333ea): 1.5s sine wave breathing pulse.
    Thinking,
    /// Neon Cyan (#06b6d4): Crisp, steady glow when moving/typing.
    Working,
    /// Warm Amber (#f59e0b): Soft shimmer pulse when settling/loading.
    Settling,
    /// Safety Red (#ef4444): Double flash alert on emergency stop.
    Emergency,
    /// Soft blue (#3b82f6): the screen is being captured.
    Capturing,
    /// Green (#22c55e): the task finished; shown for a couple of seconds, then hides itself.
    Finished,
    /// Completely transparent / hidden.
    Inactive,
}

impl Default for HudState {
    fn default() -> Self {
        Self::Inactive
    }
}

#[derive(Clone, Debug)]
struct RgbColor {
    r: u8,
    g: u8,
    b: u8,
}

impl HudState {
    fn base_color(&self) -> RgbColor {
        match self {
            Self::Thinking => RgbColor {
                r: 147,
                g: 51,
                b: 234,
            }, // Electric Purple
            Self::Working => RgbColor {
                r: 6,
                g: 182,
                b: 212,
            }, // Neon Cyan
            Self::Settling => RgbColor {
                r: 245,
                g: 158,
                b: 11,
            }, // Warm Amber
            Self::Emergency => RgbColor {
                r: 239,
                g: 68,
                b: 68,
            }, // Safety Red
            Self::Capturing => RgbColor { r: 59, g: 130, b: 246 }, // Soft Blue
            Self::Finished => RgbColor { r: 34, g: 197, b: 94 }, // Green
            Self::Inactive => RgbColor { r: 0, g: 0, b: 0 },
        }
    }

    /// The second colour that drifts along the edges with the base colour.
    fn accent_color(&self) -> [f32; 3] {
        match self {
            Self::Thinking => [236.0, 72.0, 153.0],
            Self::Working => [59.0, 130.0, 246.0],
            Self::Settling => [251.0, 113.0, 133.0],
            Self::Emergency => [249.0, 115.0, 22.0],
            Self::Capturing => [34.0, 211.0, 238.0],
            Self::Finished => [16.0, 185.0, 129.0],
            Self::Inactive => [0.0, 0.0, 0.0],
        }
    }

    /// The words shown at the top centre of the screen while this state is active.
    pub fn label(&self) -> Option<&'static str> {
        match self {
            Self::Capturing => Some("Taking screenshot"),
            Self::Thinking => Some("Thinking"),
            Self::Working => Some("Working"),
            Self::Settling => Some("Waiting for the screen"),
            Self::Emergency => Some("Stopped"),
            Self::Finished => Some("Task finished"),
            Self::Inactive => None,
        }
    }
}

/// Width the pill adds around its text: the dot on the left and the padding on the right.
const PILL_CHROME: i32 = 24 + 14 + 26;

/// How long the green "finished" state stays on screen.
const FINISHED_SHOW_SECS: f32 = 2.4;

/// Anti-aliased text drawn once by GDI into a coverage mask (0-255 per pixel).
struct LabelMask {
    width: i32,
    height: i32,
    coverage: Vec<u8>,
}

#[derive(Clone, Debug)]
struct RippleEffect {
    center_x: i32,
    center_y: i32,
    start_time: Instant,
    duration: Duration,
    max_radius: f32,
}

struct RenderContext {
    state: HudState,
    state_since: Instant,
    halo_enabled: bool,
    ripples: Vec<RippleEffect>,
}

/// Easing used by every HUD animation: slow start, slow finish (like iOS's standard curve).
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Fast start, gentle landing (for things that appear).
pub fn ease_out(t: f32) -> f32 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// How long a colour change between two states takes.
const COLOR_TRANSITION_SECS: f32 = 0.55;
/// How long the HUD takes to appear and to disappear.
const FADE_SECS: f32 = 0.45;

pub struct GlowRenderer {
    window: Arc<OverlayWindow>,
    context: Arc<Mutex<RenderContext>>,
    running: Arc<AtomicBool>,
    render_thread: Option<JoinHandle<()>>,
}

impl GlowRenderer {
    pub fn new(window: Arc<OverlayWindow>) -> Self {
        Self {
            window,
            context: Arc::new(Mutex::new(RenderContext {
                state: HudState::Inactive,
                state_since: Instant::now(),
                halo_enabled: false,
                ripples: Vec::new(),
            })),
            running: Arc::new(AtomicBool::new(false)),
            render_thread: None,
        }
    }

    /// Sets the current visual HUD state.
    pub fn set_state(&self, state: HudState) {
        if let Ok(mut ctx) = self.context.lock() {
            if ctx.state != state {
                ctx.state_since = Instant::now();
            }
            ctx.state = state;
        }

        // Hiding is done by the render loop once the glow has faded out (an eased fade, not a cut).
        if state != HudState::Inactive {
            self.window.show();
        }
    }

    /// Triggers an expanding water-ripple animation at screen point (x, y).
    pub fn trigger_click_ripple(&self, screen_x: i32, screen_y: i32) {
        let (vx, vy, _, _) = self.window.bounds();
        let local_x = screen_x - vx;
        let local_y = screen_y - vy;

        if let Ok(mut ctx) = self.context.lock() {
            ctx.ripples.push(RippleEffect {
                center_x: local_x,
                center_y: local_y,
                start_time: Instant::now(),
                duration: Duration::from_millis(350),
                max_radius: 42.0,
            });
        }
    }

    pub fn set_halo_enabled(&self, enabled: bool) {
        if let Ok(mut ctx) = self.context.lock() {
            ctx.halo_enabled = enabled;
        }
    }

    /// Starts the 60 FPS animation loop on a dedicated background thread.
    pub fn start(&mut self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);

        let window = self.window.clone();
        let context = self.context.clone();
        let running = self.running.clone();

        let width = window.width();
        let height = window.height();
        let hwnd_raw = window.hwnd() as isize;

        let handle = thread::spawn(move || {
            let hwnd = hwnd_raw as HWND;
            // Allocate DIB surface for double-buffered alpha compositing
            let (hdc_mem, hbmp, bits) = match Self::create_dib_surface(width, height) {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("Failed to create DIB surface for overlay renderer: {e}");
                    return;
                }
            };

            let pixel_count = (width * height) as usize;
            let mut label_cur: Option<(&'static str, LabelMask)> = None;
            let mut label_prev: Option<LabelMask> = None;
            let mut label_changed = Instant::now();
            let mut last_label_text: Option<&'static str> = None;
            let mut pill_w_disp: f32 = 0.0;
            let start_instant = Instant::now();
            let target_frame_dur = Duration::from_millis(16); // ~60 FPS
            // Eased state: the colour glides between states, the glow fades in and out, and the
            // brightness follows its target through a low-pass instead of jumping.
            let mut color_from = [147.0f32, 51.0, 234.0];
            let mut color_to = color_from;
            let mut color_cur = color_from;
            let mut accent_cur = [236.0f32, 72.0, 153.0];
            let mut color_started = Instant::now();
            let mut master_p: f32 = 0.0;
            let mut alpha_disp: f32 = 0.0;
            let mut last_frame = Instant::now();
            let mut hidden = true;
            let mut last_topmost = Instant::now();
            // Only what was drawn is cleared each frame (the edge bands, the pill strip, ripples),
            // not the whole screen-sized buffer: the middle of the screen is never drawn on.
            let mut drawn_spots: Vec<(i32, i32, i32, i32)> = Vec::new();
            let mut cleared_all = false;
            let mut ui = Self::ui_scale();
            let mut ui_checked = Instant::now();

            while running.load(Ordering::SeqCst) {
                let frame_start = Instant::now();
                let dt = frame_start.duration_since(last_frame).as_secs_f32().min(0.1);
                last_frame = frame_start;
                let elapsed_secs = start_instant.elapsed().as_secs_f32();

                // Snapshot current context
                let (current_state, state_age, halo_enabled, active_ripples) = {
                    let mut guard = match context.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    // Clean up finished ripples
                    guard.ripples.retain(|r| r.start_time.elapsed() < r.duration);
                    // The "finished" banner leaves by itself (the fade-out below is eased).
                    if guard.state == HudState::Finished && guard.state_since.elapsed().as_secs_f32() > FINISHED_SHOW_SECS {
                        guard.state = HudState::Inactive;
                        guard.state_since = Instant::now();
                    }
                    (guard.state, guard.state_since.elapsed().as_secs_f32(), guard.halo_enabled, guard.ripples.clone())
                };
                let _ = state_age;

                // Fade in and out with an ease-in-out curve; hide the window only once fully faded.
                let active = current_state != HudState::Inactive;
                let fade_step = dt / FADE_SECS;
                master_p = if active { (master_p + fade_step).min(1.0) } else { (master_p - fade_step).max(0.0) };
                let master = ease_in_out(master_p);
                if !active && master_p <= 0.0 {
                    if !hidden {
                        window.hide();
                        hidden = true;
                    }
                    thread::sleep(Duration::from_millis(40));
                    continue;
                }
                let just_shown = hidden;
                hidden = false;

                // The colour glides to the new state's colour (no cut between purple and cyan).
                if active {
                    let base = current_state.base_color();
                    let target = [base.r as f32, base.g as f32, base.b as f32];
                    if just_shown {
                        color_from = target;
                        color_to = target;
                    } else if target != color_to {
                        color_from = color_cur;
                        color_to = target;
                        color_started = Instant::now();
                    }
                }
                color_cur = lerp3(color_from, color_to, ease_in_out(color_started.elapsed().as_secs_f32() / COLOR_TRANSITION_SECS));
                if active {
                    let target = current_state.accent_color();
                    if just_shown {
                        accent_cur = target;
                    } else {
                        accent_cur = lerp3(accent_cur, target, 1.0 - (-dt / 0.25).exp());
                    }
                }
                let color = RgbColor { r: color_cur[0].round() as u8, g: color_cur[1].round() as u8, b: color_cur[2].round() as u8 };

                // Keep above every other window, including other always-on-top ones.
                if last_topmost.elapsed() >= Duration::from_secs(1) {
                    last_topmost = Instant::now();
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
                            hwnd,
                            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
                            0,
                            0,
                            0,
                            0,
                            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                        );
                    }
                }

                if ui_checked.elapsed() >= Duration::from_secs(2) {
                    ui_checked = Instant::now();
                    let now = Self::ui_scale();
                    if (now - ui).abs() > 0.01 {
                        ui = now;
                        last_label_text = None;
                        cleared_all = false;
                    }
                }
                // Clear what the last frame drew (the whole buffer only the first time).
                if !cleared_all {
                    unsafe { ptr::write_bytes(bits, 0, pixel_count * 4) };
                    cleared_all = true;
                } else {
                    let buffer_slice = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count) };
                    for rect in Self::fixed_regions(width, height, -window.bounds().1, ui).into_iter().chain(drawn_spots.drain(..)) {
                        Self::clear_rect(buffer_slice, width, height, rect);
                    }
                }
                let mut calm = true;

                {
                    let buffer_slice = unsafe {
                        std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count)
                    };

                    // 1. Target brightness per state (smooth breathing, no hard edges), then a
                    //    low-pass so a change of state eases instead of jumping.
                    let target_alpha = match current_state {
                        // 1.5 s breathing between 0.35 and 1.0
                        HudState::Thinking => 0.35 + 0.65 * (0.5 + 0.5 * (elapsed_secs * (2.0 * std::f32::consts::PI / 1.5)).sin()),
                        HudState::Working => 1.0,
                        HudState::Settling => 0.55 + 0.45 * (0.5 + 0.5 * (elapsed_secs * 3.2).sin()),
                        // Two soft pulses per second instead of a hard on/off flash.
                        HudState::Emergency => {
                            let cycle = elapsed_secs % 1.0;
                            let bump = |centre: f32| 0.5 * (1.0 + (std::f32::consts::PI * ((cycle - centre) / 0.13).clamp(-1.0, 1.0)).cos());
                            0.25 + 0.75 * bump(0.1).max(bump(0.4))
                        }
                        HudState::Capturing => 0.85,
                        HudState::Finished => 0.95,
                        HudState::Inactive => alpha_disp,
                    };
                    alpha_disp += (target_alpha - alpha_disp) * (1.0 - (-dt / 0.14).exp());
                    let alpha_scale = alpha_disp * master;

                    // 2. Screen-edge glow (soft quadratic falloff)
                    if alpha_scale > 0.01 {
                        Self::render_edge_glow(buffer_slice, width, height, &color_cur, &accent_cur, alpha_scale, elapsed_secs);
                    }

                    // 2b. Status pill at the top centre of the primary screen
                    if let Some(text) = current_state.label() {
                        if last_label_text != Some(text) {
                            label_prev = label_cur.take().map(|(_, mask)| mask);
                            label_cur = Self::render_label_mask(text, ui).map(|mask| (text, mask));
                            label_changed = Instant::now();
                            last_label_text = Some(text);
                        }
                    }
                    if let Some((_, mask)) = label_cur.as_ref() {
                        let (vx, vy, _, _) = window.bounds();
                        let primary_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
                        let center_x = -vx + primary_w / 2;
                        // Words crossfade and slide a few pixels; the capsule's width glides.
                        let swap = ease_in_out(label_changed.elapsed().as_secs_f32() / 0.32);
                        if swap >= 1.0 {
                            label_prev = None;
                        }
                        let target_w = mask.width as f32 + PILL_CHROME as f32 * ui;
                        if pill_w_disp < 1.0 || just_shown {
                            pill_w_disp = target_w;
                        } else {
                            pill_w_disp += (target_w - pill_w_disp) * (1.0 - (-dt / 0.10).exp());
                        }
                        let scale = 0.90 + 0.10 * master;
                        let (pill_w, pill_h) = (pill_w_disp * scale, 44.0 * ui * scale);
                        let top = -vy + (22.0 * ui) as i32 - ((1.0 - master) * 12.0 * ui) as i32;
                        let has_prev = label_prev.is_some();
                        let mut lines: Vec<(&LabelMask, f32, f32)> = Vec::new();
                        if let Some(prev) = label_prev.as_ref() {
                            lines.push((prev, 1.0 - swap, -6.0 * swap));
                        }
                        lines.push((mask, if has_prev { swap } else { 1.0 }, if has_prev { 6.0 * (1.0 - swap) } else { 0.0 }));
                        let dot_pulse = 0.85 + 0.15 * (elapsed_secs * 3.0).sin();
                        Self::render_label_pill(buffer_slice, width, height, center_x, top, pill_w, pill_h, &lines, &color_cur, master, dot_pulse, ui);
                    }

                    if label_prev.is_some() || (pill_w_disp - label_cur.as_ref().map(|(_, m)| m.width as f32 + PILL_CHROME as f32 * ui).unwrap_or(pill_w_disp)).abs() > 0.5 {
                        calm = false;
                    }

                    // 3. Render Glowing Halo Cursor
                    if halo_enabled {
                        let mut pt = POINT { x: 0, y: 0 };
                        unsafe {
                            GetCursorPos(&mut pt);
                        }
                        let (vx, vy, _, _) = window.bounds();
                        let cx = pt.x - vx;
                        let cy = pt.y - vy;

                        if cx >= 0 && cx < width && cy >= 0 && cy < height {
                            Self::render_halo_cursor(buffer_slice, width, height, cx, cy, &color, 0.85 * master);
                            drawn_spots.push((cx - 24, cy - 24, cx + 24, cy + 24));
                            calm = false;
                        }
                    }

                    // 4. Render Expanding Click Ripples
                    for ripple in &active_ripples {
                        let elapsed = ripple.start_time.elapsed().as_secs_f32();
                        let total = ripple.duration.as_secs_f32();
                        let progress = (elapsed / total).clamp(0.0, 1.0);

                        let radius = 10.0 + (ripple.max_radius - 10.0) * progress;
                        let ripple_alpha = (1.0 - ease_out(progress)) * 0.9 * master;
                        let reach = ripple.max_radius.ceil() as i32 + 8;
                        drawn_spots.push((ripple.center_x - reach, ripple.center_y - reach, ripple.center_x + reach, ripple.center_y + reach));
                        calm = false;

                        Self::render_ripple_ring(
                            buffer_slice,
                            width,
                            height,
                            ripple.center_x,
                            ripple.center_y,
                            radius,
                            &color,
                            ripple_alpha,
                        );
                    }
                }

                // 5. Blit updated surface to layered window
                let pt_dst = POINT { x: window.bounds().0, y: window.bounds().1 };
                let size = SIZE { cx: width, cy: height };
                let pt_src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION {
                    BlendOp: 0, // AC_SRC_OVER
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: 1, // AC_SRC_ALPHA
                };

                unsafe {
                    UpdateLayeredWindow(
                        hwnd,
                        ptr::null_mut(),
                        &pt_dst,
                        &size,
                        hdc_mem,
                        &pt_src,
                        0,
                        &blend,
                        ULW_ALPHA,
                    );
                }

                // Fades, colour changes, word swaps and ripples run at 60 fps; a steady glow whose
                // only motion is the slow drift and breathing looks the same at 30 and costs half.
                let changing = master_p < 1.0 || color_started.elapsed().as_secs_f32() < COLOR_TRANSITION_SECS || !active;
                let frame_dur = if calm && !changing { target_frame_dur * 2 } else { target_frame_dur };
                let elapsed_frame = frame_start.elapsed();
                if elapsed_frame < frame_dur {
                    thread::sleep(frame_dur - elapsed_frame);
                }
            }

            // Cleanup GDI allocations
            unsafe {
                DeleteObject(hbmp);
                DeleteDC(hdc_mem);
            }
        });

        self.render_thread = Some(handle);
        Ok(())
    }

    /// Stops animation loop and cleanly joins thread.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.render_thread.take() {
            let _ = handle.join();
        }
    }

    /// Creates a 32-bit top-down premultiplied ARGB DIB surface.
    fn create_dib_surface(
        width: i32,
        height: i32,
    ) -> Result<(HDC, HBITMAP, *mut u8), String> {
        unsafe {
            let hdc_mem = CreateCompatibleDC(ptr::null_mut());
            if hdc_mem.is_null() {
                return Err("CreateCompatibleDC failed".into());
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // Negative indicates top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }],
            };

            let mut bits: *mut core::ffi::c_void = ptr::null_mut();
            let hbmp = CreateDIBSection(
                hdc_mem,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                ptr::null_mut(),
                0,
            );

            if hbmp.is_null() || bits.is_null() {
                DeleteDC(hdc_mem);
                return Err("CreateDIBSection failed".into());
            }

            SelectObject(hdc_mem, hbmp);
            Ok((hdc_mem, hbmp, bits as *mut u8))
        }
    }

    /// The parts of the overlay drawn on every frame: the four edge bands and the strip at the top
    /// that holds the status pill. Everything else stays transparent unless a ripple is drawn.
    fn fixed_regions(width: i32, height: i32, pill_origin_y: i32, ui: f32) -> [(i32, i32, i32, i32); 5] {
        let g = Self::glow_depth(height).min(width / 2).min(height / 2).max(1);
        let strip = ((22.0 + 44.0) * ui + 24.0) as i32;
        [(0, 0, width, g), (0, height - g, width, height), (0, 0, g, height), (width - g, 0, width, height), (0, pill_origin_y - 16, width, pill_origin_y + strip)]
    }

    /// Display scale of the main monitor (1.0 at 100 %, 1.5 at 150 %): the status pill is drawn
    /// at that size, so it is as readable on a 4K screen as on a 1080p one.
    fn ui_scale() -> f32 {
        use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
        use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        let (mut dx, mut dy) = (96u32, 96u32);
        unsafe {
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
                return 1.0;
            }
        }
        (dx as f32 / 96.0).clamp(1.0, 3.0)
    }

    fn clear_rect(buffer: &mut [u32], width: i32, height: i32, (x0, y0, x1, y1): (i32, i32, i32, i32)) {
        let (x0, x1) = (x0.clamp(0, width), x1.clamp(0, width));
        let (y0, y1) = (y0.clamp(0, height), y1.clamp(0, height));
        if x0 >= x1 {
            return;
        }
        for y in y0..y1 {
            let row = (y * width) as usize;
            buffer[row + x0 as usize..row + x1 as usize].fill(0);
        }
    }

    /// Depth of the edge glow in pixels for a screen this tall (84 px at 1080p, more on bigger screens).
    fn glow_depth(height: i32) -> i32 {
        (84.0 * (height as f32 / 1080.0).clamp(1.0, 2.0)) as i32
    }

    /// Brightness at `d` pixels in from the edge: a bright rim that melts into a wide soft haze.
    fn glow_falloff(d: f32, depth: f32) -> f32 {
        let rim = (-d / (depth * 0.12)).exp();
        let haze = (1.0 - d / depth).max(0.0).powf(2.2);
        (0.9 * rim + 0.62 * haze).min(1.0)
    }

    /// Renders the screen-edge glow: the state colour with an accent colour drifting slowly along
    /// the edges, brightest at the edge and fading far into the screen.
    #[allow(clippy::too_many_arguments)]
    fn render_edge_glow(buffer: &mut [u32], width: i32, height: i32, color: &[f32; 3], accent: &[f32; 3], alpha_scale: f32, time: f32) {
        let g = Self::glow_depth(height).min(width / 2).min(height / 2).max(1);
        let falloff: Vec<f32> = (0..g).map(|d| Self::glow_falloff(d as f32, g as f32)).collect();
        let perimeter = 2.0 * (width + height) as f32;
        let tint = |p: f32| -> [f32; 3] {
            let k = 0.5 + 0.5 * ((p / perimeter * 2.0 - time * 0.08) * std::f32::consts::TAU).sin();
            let m = 0.6 * k;
            [color[0] + (accent[0] - color[0]) * m, color[1] + (accent[1] - color[1]) * m, color[2] + (accent[2] - color[2]) * m]
        };
        let pack = |c: &[f32; 3], a: f32| -> u32 {
            let a = a.clamp(0.0, 1.0);
            let a8 = (a * 255.0) as u32;
            let ch = |v: f32| ((v * a) as u32).min(a8);
            (a8 << 24) | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
        };
        let (w, h) = (width as f32, height as f32);
        let top: Vec<[f32; 3]> = (0..width).map(|x| tint(x as f32)).collect();
        let bottom: Vec<[f32; 3]> = (0..width).map(|x| tint(w + h + (w - 1.0 - x as f32))).collect();
        let right: Vec<[f32; 3]> = (0..height).map(|y| tint(w + y as f32)).collect();
        let left: Vec<[f32; 3]> = (0..height).map(|y| tint(2.0 * w + h + (h - 1.0 - y as f32))).collect();
        for d in 0..g {
            let a = falloff[d as usize] * alpha_scale;
            if a < 0.004 {
                continue;
            }
            let (top_row, bottom_row) = ((d * width) as usize, ((height - 1 - d) * width) as usize);
            for x in 0..width as usize {
                buffer[top_row + x] = Self::blend_max(buffer[top_row + x], pack(&top[x], a));
                buffer[bottom_row + x] = Self::blend_max(buffer[bottom_row + x], pack(&bottom[x], a));
            }
            for y in 0..height {
                let row = (y * width) as usize;
                let (l, r) = (row + d as usize, row + (width - 1 - d) as usize);
                buffer[l] = Self::blend_max(buffer[l], pack(&left[y as usize], a));
                buffer[r] = Self::blend_max(buffer[r], pack(&right[y as usize], a));
            }
        }
    }

    /// Renders anti-aliased glowing halo ring around (cx, cy).
    fn render_halo_cursor(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        cx: i32,
        cy: i32,
        color: &RgbColor,
        alpha_scale: f32,
    ) {
        let radius = 12.0f32;
        let ring_thickness = 2.5f32;
        let extent = (radius + ring_thickness + 2.0).ceil() as i32;

        let min_x = (cx - extent).max(0);
        let max_x = (cx + extent).min(width - 1);
        let min_y = (cy - extent).max(0);
        let max_y = (cy + extent).min(height - 1);

        for y in min_y..=max_y {
            let dy = (y - cy) as f32;
            for x in min_x..=max_x {
                let dx = (x - cx) as f32;
                let dist = (dx * dx + dy * dy).sqrt();
                let diff = (dist - radius).abs();

                if diff < ring_thickness {
                    let factor = 1.0 - (diff / ring_thickness);
                    let a = ((255.0 * factor * alpha_scale) as u32).min(255);
                    let px = Self::pack_premultiplied(color, a);
                    let idx = (y * width + x) as usize;
                    buffer[idx] = Self::blend_max(buffer[idx], px);
                }
            }
        }
    }

    /// Renders expanding water-ripple circle.
    fn render_ripple_ring(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        cx: i32,
        cy: i32,
        radius: f32,
        color: &RgbColor,
        alpha_scale: f32,
    ) {
        let thickness = 2.0f32;
        let extent = (radius + thickness + 1.0).ceil() as i32;

        let min_x = (cx - extent).max(0);
        let max_x = (cx + extent).min(width - 1);
        let min_y = (cy - extent).max(0);
        let max_y = (cy + extent).min(height - 1);

        for y in min_y..=max_y {
            let dy = (y - cy) as f32;
            for x in min_x..=max_x {
                let dx = (x - cx) as f32;
                let dist = (dx * dx + dy * dy).sqrt();
                let diff = (dist - radius).abs();

                if diff < thickness {
                    let factor = 1.0 - (diff / thickness);
                    let a = ((255.0 * factor * alpha_scale) as u32).min(255);
                    let px = Self::pack_premultiplied(color, a);
                    let idx = (y * width + x) as usize;
                    buffer[idx] = Self::blend_max(buffer[idx], px);
                }
            }
        }
    }

    /// Draws `text` with GDI (Segoe UI, anti-aliased) into a white-on-black bitmap and keeps the
    /// brightness as a coverage mask, because GDI cannot write the alpha channel itself.
    fn render_label_mask(text: &str, ui: f32) -> Option<LabelMask> {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateFontW, GetTextExtentPoint32W, SetBkMode, SetTextColor, TextOutW, ANTIALIASED_QUALITY, FW_SEMIBOLD,
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        // The Windows 11 display face; GDI falls back to Segoe UI where it is missing.
        let face: Vec<u16> = "Segoe UI Variable Display\0".encode_utf16().collect();
        unsafe {
            let hdc = CreateCompatibleDC(ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let font = CreateFontW(-(20.0 * ui).round() as i32, 0, 0, 0, FW_SEMIBOLD as i32, 0, 0, 0, 1, 0, 0, ANTIALIASED_QUALITY as u32, 0, face.as_ptr());
            let old_font = SelectObject(hdc, font);
            let mut size = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size);
            let (w, h) = (size.cx.max(1) + 2, size.cy.max(1) + 2);
            let (mem_dc, bitmap, bits) = match Self::create_dib_surface(w, h) {
                Ok(parts) => parts,
                Err(_) => {
                    SelectObject(hdc, old_font);
                    DeleteObject(font);
                    DeleteDC(hdc);
                    return None;
                }
            };
            let old_inner_font = SelectObject(mem_dc, font);
            ptr::write_bytes(bits, 0, (w * h) as usize * 4);
            SetBkMode(mem_dc, 1); // TRANSPARENT
            SetTextColor(mem_dc, 0x00FF_FFFF);
            TextOutW(mem_dc, 1, 1, wide.as_ptr(), wide.len() as i32);
            let pixels = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize);
            // Grey anti-aliased text on black: any colour channel is the coverage.
            let coverage: Vec<u8> = pixels.iter().map(|px| ((px >> 8) & 0xFF) as u8).collect();
            SelectObject(mem_dc, old_inner_font);
            DeleteObject(bitmap);
            DeleteDC(mem_dc);
            SelectObject(hdc, old_font);
            DeleteObject(font);
            DeleteDC(hdc);
            Some(LabelMask { width: w, height: h, coverage })
        }
    }

    /// Draws the status pill in the style of iOS's Dynamic Island: a near-black rounded capsule with
    /// a hairline edge, a softly glowing state dot and white text. `pill_w` is the animated width,
    /// `scale` the appear animation, and `lines` the text(s) to draw, each with its own opacity and
    /// vertical offset so a change of words crossfades and slides instead of switching.
    #[allow(clippy::too_many_arguments)]
    fn render_label_pill(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        center_x: i32,
        top: i32,
        pill_w: f32,
        pill_h: f32,
        lines: &[(&LabelMask, f32, f32)],
        color: &[f32; 3],
        opacity: f32,
        dot_pulse: f32,
        ui: f32,
    ) {
        if opacity <= 0.003 || pill_w < 4.0 {
            return;
        }
        let left_f = center_x as f32 - pill_w / 2.0;
        let top_f = top as f32;
        let radius = pill_h / 2.0;
        let (cx, cy) = (center_x as f32, top_f + pill_h / 2.0);
        let dot_cx = left_f + 24.0 * ui;
        let text_area_left = left_f + (24.0 + 14.0) * ui;
        let text_area_w = (pill_w - (24.0 + 14.0) * ui - 22.0 * ui).max(1.0);
        let tint_line = [255.0 * 0.7 + color[0] * 0.3, 255.0 * 0.7 + color[1] * 0.3, 255.0 * 0.7 + color[2] * 0.3];

        let x0 = (left_f.floor() as i32 - 2).max(0);
        let x1 = ((left_f + pill_w).ceil() as i32 + 2).min(width);
        let y0 = (top - 2).max(0);
        let y1 = ((top_f + pill_h).ceil() as i32 + 2).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                // Signed distance to the rounded rectangle (negative inside), anti-aliased edge.
                let px = x as f32 + 0.5 - cx;
                let py = y as f32 + 0.5 - cy;
                let qx = px.abs() - (pill_w / 2.0 - radius);
                let qy = py.abs() - (pill_h / 2.0 - radius);
                let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                if dist > 1.0 {
                    continue;
                }
                let edge = (0.5 - dist).clamp(0.0, 1.0);
                // Body: near-black; a one pixel light hairline just inside the edge.
                let hairline = (1.0 - (-dist - 0.5).abs().min(1.0)).clamp(0.0, 1.0) * 0.22;
                let (mut r, mut g, mut b) = (10.0f32, 10.0f32, 12.0f32);
                r += (tint_line[0] - r) * hairline;
                g += (tint_line[1] - g) * hairline;
                b += (tint_line[2] - b) * hairline;
                let mut a = 0.93 * opacity * edge;

                // State dot with a soft halo.
                let ddx = x as f32 + 0.5 - dot_cx;
                let ddy = y as f32 + 0.5 - cy;
                let dd = ddx.hypot(ddy);
                if dd < 11.0 * ui {
                    let core = (5.5 * ui - dd).clamp(0.0, 1.0);
                    let halo = (1.0 - (dd / (11.0 * ui))).clamp(0.0, 1.0).powi(2) * 0.38 * dot_pulse;
                    let cover = (core * (0.55 + 0.45 * dot_pulse) + halo).clamp(0.0, 1.0);
                    r = r * (1.0 - cover) + color[0] * cover;
                    g = g * (1.0 - cover) + color[1] * cover;
                    b = b * (1.0 - cover) + color[2] * cover;
                }

                // Text, vertically centred, left aligned after the dot; clipped to the capsule.
                for (mask, line_opacity, y_offset) in lines {
                    if *line_opacity <= 0.003 {
                        continue;
                    }
                    let text_left = text_area_left;
                    let text_top = cy - mask.height as f32 / 2.0 + y_offset;
                    let tx = (x as f32 - text_left).floor() as i32;
                    let ty = (y as f32 - text_top).floor() as i32;
                    if tx >= 0 && ty >= 0 && tx < mask.width && ty < mask.height && (tx as f32) < text_area_w + 4.0 {
                        // While the capsule is still narrower than its words (it is growing), the
                        // words fade at the edge instead of being cut; at full width nothing fades.
                        let tail = ((text_area_w + 4.0 - tx as f32) / (8.0 * ui)).clamp(0.0, 1.0);
                        let cover = mask.coverage[(ty * mask.width + tx) as usize] as f32 / 255.0 * line_opacity * opacity * tail;
                        r = r * (1.0 - cover) + 255.0 * cover;
                        g = g * (1.0 - cover) + 255.0 * cover;
                        b = b * (1.0 - cover) + 255.0 * cover;
                        a += (1.0 - a) * cover;
                    }
                }

                let a = a.clamp(0.0, 1.0);
                let a8 = (a * 255.0) as u32;
                let pr = ((r.clamp(0.0, 255.0) * a) as u32).min(a8);
                let pg = ((g.clamp(0.0, 255.0) * a) as u32).min(a8);
                let pb = ((b.clamp(0.0, 255.0) * a) as u32).min(a8);
                let idx = (y * width + x) as usize;
                buffer[idx] = (a8 << 24) | (pr << 16) | (pg << 8) | pb;
            }
        }
    }

    /// Converts RGB and Alpha into 32-bit BGRA premultiplied format: (A << 24) | (R' << 16) | (G' << 8) | B'
    #[inline(always)]
    fn pack_premultiplied(color: &RgbColor, a: u32) -> u32 {
        let r = ((color.r as u32 * a) / 255) & 0xFF;
        let g = ((color.g as u32 * a) / 255) & 0xFF;
        let b = ((color.b as u32 * a) / 255) & 0xFF;
        (a << 24) | (r << 16) | (g << 8) | b
    }

    /// Blends two premultiplied pixels keeping maximum channel values for smooth glow overlap.
    #[inline(always)]
    fn blend_max(orig: u32, new_px: u32) -> u32 {
        let a1 = (orig >> 24) & 0xFF;
        let r1 = (orig >> 16) & 0xFF;
        let g1 = (orig >> 8) & 0xFF;
        let b1 = orig & 0xFF;

        let a2 = (new_px >> 24) & 0xFF;
        let r2 = (new_px >> 16) & 0xFF;
        let g2 = (new_px >> 8) & 0xFF;
        let b2 = new_px & 0xFF;

        let a = a1.max(a2);
        let r = r1.max(r2);
        let g = g1.max(g2);
        let b = b1.max(b2);

        (a << 24) | (r << 16) | (g << 8) | b
    }
}

impl Drop for GlowRenderer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_premultiplied_color_packing() {
        let color = RgbColor {
            r: 255,
            g: 100,
            b: 50,
        };
        let packed = GlowRenderer::pack_premultiplied(&color, 128);
        let a = (packed >> 24) & 0xFF;
        let r = (packed >> 16) & 0xFF;
        let g = (packed >> 8) & 0xFF;
        let b = packed & 0xFF;

        assert_eq!(a, 128);
        assert_eq!(r, (255 * 128) / 255);
        assert_eq!(g, (100 * 128) / 255);
        assert_eq!(b, (50 * 128) / 255);
    }

    #[test]
    fn test_hud_state_colors() {
        assert_eq!(HudState::Thinking.base_color().r, 147);
        assert_eq!(HudState::Working.base_color().g, 182);
        assert_eq!(HudState::Settling.base_color().r, 245);
        assert_eq!(HudState::Emergency.base_color().r, 239);
        assert_eq!(HudState::Capturing.base_color().b, 246);
        assert_eq!(HudState::Finished.base_color().g, 197);
    }

    #[test]
    fn every_active_state_has_words_for_the_screen() {
        assert_eq!(HudState::Capturing.label(), Some("Taking screenshot"));
        assert_eq!(HudState::Thinking.label(), Some("Thinking"));
        assert_eq!(HudState::Working.label(), Some("Working"));
        assert_eq!(HudState::Finished.label(), Some("Task finished"));
        assert_eq!(HudState::Inactive.label(), None);
    }

    #[test]
    fn the_label_text_is_drawn_into_a_mask_and_the_pill_paints_pixels() {
        let Some(mask) = GlowRenderer::render_label_mask("Taking screenshot", 1.0) else {
            println!("no GDI in this environment, skipping");
            return;
        };
        assert!(mask.width > 60 && mask.height > 10);
        assert!(mask.coverage.iter().any(|c| *c > 200), "the text must produce solid pixels");
        let (w, h) = (600, 120);
        let mut buffer = vec![0u32; (w * h) as usize];
        let c = HudState::Thinking.base_color();
        let color = [c.r as f32, c.g as f32, c.b as f32];
        let lines = [(&mask, 1.0f32, 0.0f32)];
        GlowRenderer::render_label_pill(&mut buffer, w, h, 300, 20, (mask.width + 60) as f32, 44.0, &lines, &color, 1.0, 1.0, 1.0);
        let painted = buffer.iter().filter(|px| (*px >> 24) > 0).count();
        assert!(painted > 2000, "the pill should cover a few thousand pixels, got {painted}");
        // Fully faded out it paints nothing, and halfway it is partly transparent.
        let mut empty = vec![0u32; (w * h) as usize];
        GlowRenderer::render_label_pill(&mut empty, w, h, 300, 20, (mask.width + 60) as f32, 44.0, &lines, &color, 0.0, 1.0, 1.0);
        assert!(empty.iter().all(|px| *px == 0));
        // Premultiplied: no colour channel may exceed alpha.
        assert!(buffer.iter().all(|px| ((px >> 16) & 0xFF) <= (px >> 24) && ((px >> 8) & 0xFF) <= (px >> 24) && (px & 0xFF) <= (px >> 24)));
    }

    #[test]
    fn easing_curves_start_and_end_gently_and_colours_blend_smoothly() {
        assert_eq!((ease_in_out(0.0), ease_in_out(1.0)), (0.0, 1.0));
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        assert!(ease_in_out(0.1) < 0.1 && ease_in_out(0.9) > 0.9, "slow at both ends");
        assert!(ease_out(0.2) > 0.2, "fast start");
        // Purple to cyan passes through intermediate colours, never jumping.
        let (purple, cyan) = ([147.0, 51.0, 234.0], [6.0, 182.0, 212.0]);
        let mut last = lerp3(purple, cyan, 0.0);
        for i in 1..=20 {
            let now = lerp3(purple, cyan, ease_in_out(i as f32 / 20.0));
            assert!((now[0] - last[0]).abs() < 40.0, "no jump between frames");
            last = now;
        }
        assert_eq!(last, cyan);
    }

    #[test]
    fn test_glow_renderer_lifecycle() {
        let window = match OverlayWindow::new() {
            Ok(w) => Arc::new(w),
            Err(e) => {
                println!("Skipping renderer lifecycle test in sandbox: {e}");
                return;
            }
        };

        let mut renderer = GlowRenderer::new(window);
        renderer.set_state(HudState::Thinking);
        assert!(renderer.start().is_ok());

        renderer.trigger_click_ripple(500, 500);
        thread::sleep(Duration::from_millis(50));

        renderer.set_state(HudState::Working);
        thread::sleep(Duration::from_millis(50));

        renderer.set_state(HudState::Inactive);
        renderer.stop();
    }

    /// Writes what the overlay draws (edge glow, status pill, working pointer) over a grey
    /// "desktop" to AURA_PREVIEW_PNG, for a person to look at: the real overlay is excluded from
    /// screenshots. Run with --ignored.
    #[test]
    #[ignore]
    fn render_preview_frame() {
        let Ok(path) = std::env::var("AURA_PREVIEW_PNG") else { panic!("set AURA_PREVIEW_PNG") };
        let (w, h) = (1600i32, 900i32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let state = HudState::Thinking;
        let base = state.base_color();
        let color = [base.r as f32, base.g as f32, base.b as f32];
        GlowRenderer::render_edge_glow(&mut buffer, w, h, &color, &state.accent_color(), 1.0, 1.3);
        let mask = GlowRenderer::render_label_mask("Thinking", 1.0).unwrap();
        let lines = vec![(&mask, 1.0f32, 0.0f32)];
        GlowRenderer::render_label_pill(&mut buffer, w, h, w / 2, 22, (mask.width + PILL_CHROME) as f32, 44.0, &lines, &color, 1.0, 1.0, 1.0);
        // Composite over a mid-grey desktop with a white window, plus the pointer at (800, 450).
        let mut image = image::RgbaImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let window = (300..1300).contains(&x) && (200..760).contains(&y);
                let bg = if window { [250.0, 250.0, 250.0] } else { [58.0, 66.0, 82.0] };
                let px = buffer[(y * w + x) as usize];
                let a = ((px >> 24) & 0xFF) as f32 / 255.0;
                let c = [((px >> 16) & 0xFF) as f32, ((px >> 8) & 0xFF) as f32, (px & 0xFF) as f32];
                let out = [0, 1, 2].map(|i| (c[i] + bg[i] * (1.0 - a)).min(255.0) as u8);
                image.put_pixel(x as u32, y as u32, image::Rgba([out[0], out[1], out[2], 255]));
            }
        }
        let size = 48;
        let arrow = crate::overlay::cursor::arrow_pixels(size);
        for (i, [r, g, b, a]) in arrow.into_iter().enumerate() {
            let (x, y) = (800 + i as u32 % size as u32, 450 + i as u32 / size as u32);
            let under = *image.get_pixel(x, y);
            let a = a as f32 / 255.0;
            let mix = |c: u8, u: u8| (c as f32 * a + u as f32 * (1.0 - a)) as u8;
            image.put_pixel(x, y, image::Rgba([mix(r, under[0]), mix(g, under[1]), mix(b, under[2]), 255]));
        }
        image.save(&path).unwrap();
    }
}
