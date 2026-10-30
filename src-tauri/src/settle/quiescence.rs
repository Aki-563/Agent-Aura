//! Optical Frame Quiescence Detector (Phase 3 implementation)
//!
//! Solves the Render Race Condition by verifying that visual updates have fully settled
//! before screenshots are submitted to the cognitive vision pipeline.
//!
//! Implements the 3-step pipeline specified in docs/CORE.md Section 5:
//! 1. Mandatory Input Ingestion (60ms event queue drainage)
//! 2. Hardware Cursor Inspection (GetCursorInfo IDC_WAIT / IDC_APPSTARTING)
//! 3. Perceptual Frame Difference Quiescence (Delta < 0.2% over 50ms downsampled luminance)

use std::thread;
use std::time::{Duration, Instant};

use crate::capture::dxgi::DxgiCapture;
use crate::settle::cursor::CursorMonitor;

pub const DOWNSAMPLE_WIDTH: usize = 64;
pub const DOWNSAMPLE_HEIGHT: usize = 36;
pub const QUIESCENCE_THRESHOLD: f32 = 0.002; // 0.2% mean variation

#[derive(Debug, Clone)]
pub struct SettleResult {
    /// Whether the display achieved optical quiescence before timeout.
    pub settled: bool,
    /// Final measured mean absolute frame delta (0.0 to 1.0).
    pub delta: f32,
    /// Total duration spent waiting for settle in milliseconds.
    pub elapsed_ms: u64,
    /// Number of optical frame comparisons performed.
    pub iterations: u32,
    /// Whether the hardware cursor was actively showing a busy spinner during settle.
    pub cursor_was_busy: bool,
}

pub struct QuiescenceDetector;

impl QuiescenceDetector {
    /// Downsamples a full-resolution RGBA image buffer into a compact 64x36 luminance matrix.
    ///
    /// Computes perceived brightness using standard ITU-R BT.601 coefficients:
    /// Y = (0.299 * R + 0.587 * G + 0.114 * B) / 255.0
    pub fn downsample_to_luminance(
        rgba_data: &[u8],
        src_width: u32,
        src_height: u32,
        target_width: usize,
        target_height: usize,
    ) -> Vec<f32> {
        let mut luma = Vec::with_capacity(target_width * target_height);
        let x_scale = src_width as f32 / target_width as f32;
        let y_scale = src_height as f32 / target_height as f32;

        for ty in 0..target_height {
            let sy = ((ty as f32 * y_scale) as u32).min(src_height - 1);
            let row_offset = (sy * src_width * 4) as usize;

            for tx in 0..target_width {
                let sx = ((tx as f32 * x_scale) as u32).min(src_width - 1);
                let px_offset = row_offset + (sx * 4) as usize;

                if px_offset + 3 < rgba_data.len() {
                    let r = rgba_data[px_offset] as f32;
                    let g = rgba_data[px_offset + 1] as f32;
                    let b = rgba_data[px_offset + 2] as f32;
                    let y = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
                    luma.push(y);
                } else {
                    luma.push(0.0);
                }
            }
        }

        luma
    }

    /// Computes the Mean Absolute Difference between two normalized luminance buffers:
    /// Delta = 1/N * sum(|Y1(i) - Y2(i)|)
    pub fn compute_delta(luma_a: &[f32], luma_b: &[f32]) -> f32 {
        if luma_a.is_empty() || luma_a.len() != luma_b.len() {
            return 1.0;
        }

        let total_diff: f32 = luma_a
            .iter()
            .zip(luma_b.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();

        total_diff / (luma_a.len() as f32)
    }

    /// Executes the full Settle Quiescence pipeline against the active DXGI capture instance.
    pub fn wait_for_settle(
        capture: &mut DxgiCapture,
        max_timeout_ms: u64,
    ) -> Result<SettleResult, String> {
        let start_time = Instant::now();
        let timeout = Duration::from_millis(max_timeout_ms);

        // Step 1: Mandatory input ingestion delay (60ms for Windows message queue dispatch)
        thread::sleep(Duration::from_millis(60));

        // Step 2: Hardware Cursor State Poll (wait up to 1500ms if IDC_WAIT or IDC_APPSTARTING)
        let cursor_busy_init = CursorMonitor::is_system_busy();
        let mut cursor_was_busy = cursor_busy_init;

        if cursor_busy_init {
            let cursor_settled = CursorMonitor::wait_for_cursor_idle(
                Duration::from_millis(1500),
                Duration::from_millis(40),
            );
            if !cursor_settled {
                cursor_was_busy = true;
            }
        }

        // Step 3: Optical Frame Quiescence Loop
        let mut iterations = 0;
        let mut last_delta = 1.0;

        // Capture initial baseline frame
        let initial_frame = capture
            .capture_frame(100)
            .map_err(|e| format!("Baseline DXGI capture failed: {e}"))?;

        let mut prev_luma = Self::downsample_to_luminance(
            &initial_frame.data,
            initial_frame.width,
            initial_frame.height,
            DOWNSAMPLE_WIDTH,
            DOWNSAMPLE_HEIGHT,
        );

        while start_time.elapsed() < timeout {
            iterations += 1;
            // Inter-frame sampling interval (50ms)
            thread::sleep(Duration::from_millis(50));

            let current_frame = match capture.capture_frame(80) {
                Ok(f) => f,
                Err(_e) => {
                    // In static desktop mode, DXGI timeout indicates zero pixel changes (perfect quiescence!)
                    let elapsed = start_time.elapsed().as_millis() as u64;
                    return Ok(SettleResult {
                        settled: true,
                        delta: 0.0,
                        elapsed_ms: elapsed,
                        iterations,
                        cursor_was_busy,
                    });
                }
            };

            let curr_luma = Self::downsample_to_luminance(
                &current_frame.data,
                current_frame.width,
                current_frame.height,
                DOWNSAMPLE_WIDTH,
                DOWNSAMPLE_HEIGHT,
            );

            let delta = Self::compute_delta(&prev_luma, &curr_luma);
            last_delta = delta;

            if delta <= QUIESCENCE_THRESHOLD {
                // Screen has settled (less than 0.2% variation)
                let elapsed = start_time.elapsed().as_millis() as u64;
                return Ok(SettleResult {
                    settled: true,
                    delta,
                    elapsed_ms: elapsed,
                    iterations,
                    cursor_was_busy,
                });
            }

            prev_luma = curr_luma;
        }

        let elapsed = start_time.elapsed().as_millis() as u64;
        Ok(SettleResult {
            settled: false,
            delta: last_delta,
            elapsed_ms: elapsed,
            iterations,
            cursor_was_busy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_luminance_downsample_and_delta() {
        let width = 640u32;
        let height = 360u32;
        let total_pixels = (width * height) as usize;

        // Frame 1: solid white (R=255, G=255, B=255, A=255)
        let frame_white = vec![255u8; total_pixels * 4];
        let luma_white = QuiescenceDetector::downsample_to_luminance(
            &frame_white,
            width,
            height,
            DOWNSAMPLE_WIDTH,
            DOWNSAMPLE_HEIGHT,
        );

        assert_eq!(luma_white.len(), DOWNSAMPLE_WIDTH * DOWNSAMPLE_HEIGHT);
        for &val in &luma_white {
            assert!((val - 1.0).abs() < 0.01);
        }

        // Frame 2: identical white frame -> delta should be 0.0
        let delta_identical = QuiescenceDetector::compute_delta(&luma_white, &luma_white);
        assert_eq!(delta_identical, 0.0);

        // Frame 3: solid black (R=0, G=0, B=0, A=255)
        let mut frame_black = vec![0u8; total_pixels * 4];
        for i in (3..frame_black.len()).step_by(4) {
            frame_black[i] = 255;
        }
        let luma_black = QuiescenceDetector::downsample_to_luminance(
            &frame_black,
            width,
            height,
            DOWNSAMPLE_WIDTH,
            DOWNSAMPLE_HEIGHT,
        );

        let delta_inverted = QuiescenceDetector::compute_delta(&luma_white, &luma_black);
        assert!((delta_inverted - 1.0).abs() < 0.01);
        assert!(delta_inverted > QUIESCENCE_THRESHOLD);
    }
}
