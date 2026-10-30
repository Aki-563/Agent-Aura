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
                halo_enabled: true,
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

        if state == HudState::Inactive {
            self.window.hide();
        } else {
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
            let mut label_cache: Option<(&'static str, LabelMask)> = None;
            let start_instant = Instant::now();
            let target_frame_dur = Duration::from_millis(16); // ~60 FPS

            while running.load(Ordering::SeqCst) {
                let frame_start = Instant::now();
                let elapsed_secs = start_instant.elapsed().as_secs_f32();

                // Snapshot current context
                let mut hide_now = false;
                let (current_state, state_age, halo_enabled, active_ripples) = {
                    let mut guard = match context.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    // Clean up finished ripples
                    guard.ripples.retain(|r| r.start_time.elapsed() < r.duration);
                    // The "finished" banner hides itself.
                    if guard.state == HudState::Finished && guard.state_since.elapsed().as_secs_f32() > FINISHED_SHOW_SECS {
                        guard.state = HudState::Inactive;
                        guard.state_since = Instant::now();
                        hide_now = true;
                    }
                    (guard.state, guard.state_since.elapsed().as_secs_f32(), guard.halo_enabled, guard.ripples.clone())
                };
                if hide_now {
                    window.hide();
                }

                // Clear premultiplied ARGB buffer to zero
                unsafe {
                    ptr::write_bytes(bits, 0, pixel_count * 4);
                }

                if current_state != HudState::Inactive {
                    let buffer_slice = unsafe {
                        std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count)
                    };

                    // 1. Calculate breathing alpha modulation based on state
                    let (alpha_scale, color) = match current_state {
                        HudState::Thinking => {
                            // 1.5s sine-wave breathing pulse (cycles between 0.35 and 1.0)
                            let sine = (elapsed_secs * (2.0 * std::f32::consts::PI / 1.5)).sin();
                            let alpha = 0.35 + 0.65 * (0.5 + 0.5 * sine);
                            (alpha, current_state.base_color())
                        }
                        HudState::Working => (1.0, current_state.base_color()),
                        HudState::Settling => {
                            // Soft 0.8s shimmering pulse
                            let shimmer = (elapsed_secs * 6.0).sin();
                            let alpha = 0.55 + 0.45 * (0.5 + 0.5 * shimmer);
                            (alpha, current_state.base_color())
                        }
                        HudState::Emergency => {
                            // Rapid double-flash pattern
                            let cycle = elapsed_secs % 1.0;
                            let alpha = if (cycle > 0.0 && cycle < 0.18) || (cycle > 0.28 && cycle < 0.46) {
                                1.0
                            } else {
                                0.0
                            };
                            (alpha, current_state.base_color())
                        }
                        HudState::Capturing => (0.85, current_state.base_color()),
                        HudState::Finished => {
                            // Bright, then fading out over the last second.
                            let fade = (FINISHED_SHOW_SECS - state_age).clamp(0.0, 1.0);
                            (0.95 * fade, current_state.base_color())
                        }
                        HudState::Inactive => (0.0, current_state.base_color()),
                    };

                    // 2. Render Screen-Edge Glow (14px gradient border)
                    if alpha_scale > 0.01 {
                        Self::render_edge_glow(buffer_slice, width, height, &color, 14, alpha_scale);
                    }

                    // 2b. Status label at the top centre of the primary screen
                    if let Some(text) = current_state.label() {
                        if label_cache.as_ref().map(|(cached, _)| *cached != text).unwrap_or(true) {
                            label_cache = Self::render_label_mask(text).map(|mask| (text, mask));
                        }
                        if let Some((_, mask)) = label_cache.as_ref() {
                            let (vx, vy, _, _) = window.bounds();
                            let primary_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
                            let center_x = -vx + primary_w / 2;
                            let appear = (state_age / 0.18).clamp(0.0, 1.0);
                            let fade_out = if current_state == HudState::Finished {
                                ((FINISHED_SHOW_SECS - state_age) / 0.8).clamp(0.0, 1.0)
                            } else {
                                1.0
                            };
                            let pulse = if current_state == HudState::Working { 1.0 } else { 0.75 + 0.25 * (elapsed_secs * 4.0).sin().abs() };
                            Self::render_label_pill(buffer_slice, width, height, center_x, -vy + 26, mask, &color, appear * fade_out, pulse);
                        }
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
                            Self::render_halo_cursor(buffer_slice, width, height, cx, cy, &color, 0.85);
                        }
                    }

                    // 4. Render Expanding Click Ripples
                    for ripple in &active_ripples {
                        let elapsed = ripple.start_time.elapsed().as_secs_f32();
                        let total = ripple.duration.as_secs_f32();
                        let progress = (elapsed / total).clamp(0.0, 1.0);

                        let radius = 10.0 + (ripple.max_radius - 10.0) * progress;
                        let ripple_alpha = (1.0 - progress) * 0.9;

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

                let elapsed_frame = frame_start.elapsed();
                if elapsed_frame < target_frame_dur {
                    thread::sleep(target_frame_dur - elapsed_frame);
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

    /// Renders screen-edge glow along display boundaries.
    fn render_edge_glow(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        color: &RgbColor,
        glow_size: i32,
        alpha_scale: f32,
    ) {
        let g = glow_size;

        // Top border
        for y in 0..g.min(height) {
            let factor = 1.0 - (y as f32 / g as f32);
            let a = ((255.0 * factor * alpha_scale) as u32).min(255);
            let px = Self::pack_premultiplied(color, a);
            for x in 0..width {
                let idx = (y * width + x) as usize;
                buffer[idx] = Self::blend_max(buffer[idx], px);
            }
        }

        // Bottom border
        for y in 0..g.min(height) {
            let row = height - 1 - y;
            let factor = 1.0 - (y as f32 / g as f32);
            let a = ((255.0 * factor * alpha_scale) as u32).min(255);
            let px = Self::pack_premultiplied(color, a);
            for x in 0..width {
                let idx = (row * width + x) as usize;
                buffer[idx] = Self::blend_max(buffer[idx], px);
            }
        }

        // Left border
        for x in 0..g.min(width) {
            let factor = 1.0 - (x as f32 / g as f32);
            let a = ((255.0 * factor * alpha_scale) as u32).min(255);
            let px = Self::pack_premultiplied(color, a);
            for y in 0..height {
                let idx = (y * width + x) as usize;
                buffer[idx] = Self::blend_max(buffer[idx], px);
            }
        }

        // Right border
        for x in 0..g.min(width) {
            let col = width - 1 - x;
            let factor = 1.0 - (x as f32 / g as f32);
            let a = ((255.0 * factor * alpha_scale) as u32).min(255);
            let px = Self::pack_premultiplied(color, a);
            for y in 0..height {
                let idx = (y * width + col) as usize;
                buffer[idx] = Self::blend_max(buffer[idx], px);
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
    fn render_label_mask(text: &str) -> Option<LabelMask> {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateFontW, GetTextExtentPoint32W, SetBkMode, SetTextColor, TextOutW, ANTIALIASED_QUALITY, FW_SEMIBOLD,
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        unsafe {
            let hdc = CreateCompatibleDC(ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let font = CreateFontW(-24, 0, 0, 0, FW_SEMIBOLD as i32, 0, 0, 0, 1, 0, 0, ANTIALIASED_QUALITY as u32, 0, face.as_ptr());
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

    /// Draws the status pill: a dark rounded bar with a state-coloured border and dot, and the text.
    #[allow(clippy::too_many_arguments)]
    fn render_label_pill(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        center_x: i32,
        top: i32,
        mask: &LabelMask,
        color: &RgbColor,
        opacity: f32,
        dot_pulse: f32,
    ) {
        let pad_x = 26;
        let pad_y = 11;
        let dot_room = 26;
        let pill_w = mask.width + pad_x * 2 + dot_room;
        let pill_h = mask.height + pad_y * 2;
        let left = center_x - pill_w / 2;
        let radius = pill_h as f32 / 2.0;
        let text_left = left + pad_x + dot_room;
        let text_top = top + pad_y;
        let dot_cx = (left + pad_x + 6) as f32;
        let dot_cy = top as f32 + pill_h as f32 / 2.0;

        for y in top.max(0)..(top + pill_h).min(height) {
            for x in left.max(0)..(left + pill_w).min(width) {
                // Signed distance to the rounded rectangle (negative inside).
                let px = x as f32 + 0.5 - (left as f32 + pill_w as f32 / 2.0);
                let py = y as f32 + 0.5 - (top as f32 + pill_h as f32 / 2.0);
                let qx = px.abs() - (pill_w as f32 / 2.0 - radius);
                let qy = py.abs() - (pill_h as f32 / 2.0 - radius);
                let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                if dist > 0.5 {
                    continue;
                }
                let edge = (0.5 - dist).clamp(0.0, 1.0); // anti-aliased outline
                // Dark body with a 2 px state-coloured border.
                let border = (1.0 - ((-dist - 1.0) / 1.5).clamp(0.0, 1.0)) * 0.9;
                let body_a = 0.84 * opacity * edge;
                let (br, bg, bb) = (12.0f32, 14.0f32, 26.0f32);
                let mut r = br * (1.0 - border) + color.r as f32 * border;
                let mut g = bg * (1.0 - border) + color.g as f32 * border;
                let mut b = bb * (1.0 - border) + color.b as f32 * border;
                let mut a = body_a + (1.0 - body_a) * border * opacity * edge * 0.55;

                // Status dot
                let ddx = x as f32 + 0.5 - dot_cx;
                let ddy = y as f32 + 0.5 - dot_cy;
                let dd = ddx.hypot(ddy);
                if dd < 6.5 {
                    let cover = (6.5 - dd).clamp(0.0, 1.0) * dot_pulse;
                    r = r * (1.0 - cover) + color.r as f32 * cover;
                    g = g * (1.0 - cover) + color.g as f32 * cover;
                    b = b * (1.0 - cover) + color.b as f32 * cover;
                    a += (1.0 - a) * cover * opacity;
                }

                // Text (white) over the body
                let tx = x - text_left;
                let ty = y - text_top;
                if tx >= 0 && ty >= 0 && tx < mask.width && ty < mask.height {
                    let cover = mask.coverage[(ty * mask.width + tx) as usize] as f32 / 255.0 * opacity;
                    r = r * (1.0 - cover) + 255.0 * cover;
                    g = g * (1.0 - cover) + 255.0 * cover;
                    b = b * (1.0 - cover) + 255.0 * cover;
                    a += (1.0 - a) * cover;
                }

                let a = a.clamp(0.0, 1.0);
                let a8 = (a * 255.0) as u32;
                let pr = ((r.clamp(0.0, 255.0) * a) as u32).min(a8);
                let pg = ((g.clamp(0.0, 255.0) * a) as u32).min(a8);
                let pb = ((b.clamp(0.0, 255.0) * a) as u32).min(a8);
                let idx = (y * width + x) as usize;
                // Painted over whatever the glow put there.
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
        let Some(mask) = GlowRenderer::render_label_mask("Taking screenshot") else {
            println!("no GDI in this environment, skipping");
            return;
        };
        assert!(mask.width > 60 && mask.height > 10);
        assert!(mask.coverage.iter().any(|c| *c > 200), "the text must produce solid pixels");
        let (w, h) = (600, 120);
        let mut buffer = vec![0u32; (w * h) as usize];
        let color = HudState::Thinking.base_color();
        GlowRenderer::render_label_pill(&mut buffer, w, h, 300, 20, &mask, &color, 1.0, 1.0);
        let painted = buffer.iter().filter(|px| (*px >> 24) > 0).count();
        assert!(painted > 2000, "the pill should cover a few thousand pixels, got {painted}");
        // Premultiplied: no colour channel may exceed alpha.
        assert!(buffer.iter().all(|px| ((px >> 16) & 0xFF) <= (px >> 24) && ((px >> 8) & 0xFF) <= (px >> 24) && (px & 0xFF) <= (px >> 24)));
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
}
