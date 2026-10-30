//! Local 2ms Radar Tracker (Phase 3 implementation)
//!
//! Solves the "Moved Icon" problem without burning API roundtrips by running
//! ultra-fast Normalized Cross-Correlation (NCC) template matching over the frame.
//!
//! Performance Target: Executes in <= 2ms on CPU within Region of Interest (ROI).
//! Confidence Threshold: >= 0.88.

use std::time::Instant;

pub const DEFAULT_RADAR_CONFIDENCE_THRESHOLD: f32 = 0.88;

#[derive(Debug, Clone, Copy)]
pub struct SearchRoi {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct MatchResult {
    /// Whether an anchor match was found meeting the confidence threshold.
    pub matched: bool,
    /// Detected center X coordinate in screen space.
    pub x: i32,
    /// Detected center Y coordinate in screen space.
    pub y: i32,
    /// NCC match confidence (0.0 to 1.0).
    pub confidence: f32,
    /// Execution time in microseconds.
    pub duration_us: u128,
}

pub struct RadarTracker;

impl RadarTracker {
    /// Converts packed RGBA buffer to 8-bit grayscale luminance.
    pub fn to_grayscale(rgba: &[u8], width: u32, height: u32) -> Vec<f32> {
        let count = (width * height) as usize;
        let mut gray = Vec::with_capacity(count);

        for px in rgba.chunks_exact(4) {
            let r = px[0] as f32;
            let g = px[1] as f32;
            let b = px[2] as f32;
            gray.push(0.299 * r + 0.587 * g + 0.114 * b);
        }

        gray
    }

    /// Searches for a visual template inside the search image using Normalized Cross-Correlation (NCC).
    ///
    /// If an `roi` (Region of Interest) is provided, searching is constrained to that window,
    /// achieving sub-2ms execution for relocated buttons and icons.
    pub fn find_template(
        search_rgba: &[u8],
        search_w: u32,
        search_h: u32,
        template_rgba: &[u8],
        template_w: u32,
        template_h: u32,
        roi: Option<SearchRoi>,
        threshold: f32,
    ) -> MatchResult {
        let start = Instant::now();

        if template_w == 0 || template_h == 0 || template_w > search_w || template_h > search_h {
            return MatchResult {
                matched: false,
                x: 0,
                y: 0,
                confidence: 0.0,
                duration_us: start.elapsed().as_micros(),
            };
        }

        let search_gray = Self::to_grayscale(search_rgba, search_w, search_h);
        let templ_gray = Self::to_grayscale(template_rgba, template_w, template_h);

        let t_count = templ_gray.len() as f32;
        let t_mean = templ_gray.iter().sum::<f32>() / t_count;

        // Precompute template zero-mean and variance
        let mut templ_zero_mean = Vec::with_capacity(templ_gray.len());
        let mut templ_sq_sum = 0.0f32;
        for &val in &templ_gray {
            let diff = val - t_mean;
            templ_zero_mean.push(diff);
            templ_sq_sum += diff * diff;
        }

        let templ_norm = templ_sq_sum.sqrt();
        if templ_norm < 1e-4 {
            // Template has zero variance (flat uniform color)
            return MatchResult {
                matched: false,
                x: 0,
                y: 0,
                confidence: 0.0,
                duration_us: start.elapsed().as_micros(),
            };
        }

        // Determine effective search bounds
        let (start_x, start_y, end_x, end_y) = if let Some(r) = roi {
            let sx = (r.x.max(0) as u32).min(search_w - template_w);
            let sy = (r.y.max(0) as u32).min(search_h - template_h);
            let ex = ((r.x + r.width as i32).max(0) as u32).min(search_w - template_w);
            let ey = ((r.y + r.height as i32).max(0) as u32).min(search_h - template_h);
            (sx, sy, ex.max(sx), ey.max(sy))
        } else {
            (0, 0, search_w - template_w, search_h - template_h)
        };

        let mut best_score = -1.0f32;
        let mut best_pos = (0u32, 0u32);

        let sw = search_w as usize;
        let tw = template_w as usize;
        let th = template_h as usize;

        // Slide template over search region with single-pass NCC
        let inv_t_count = 1.0f32 / t_count;
        for sy in start_y..=end_y {
            for sx in start_x..=end_x {
                let mut cross_prod = 0.0f32;
                let mut patch_sum = 0.0f32;
                let mut patch_sq_sum_raw = 0.0f32;

                for ty in 0..th {
                    let search_row = (sy as usize + ty) * sw + sx as usize;
                    let templ_row = ty * tw;

                    for tx in 0..tw {
                        let p = search_gray[search_row + tx];
                        let t = templ_zero_mean[templ_row + tx];

                        cross_prod += p * t;
                        patch_sum += p;
                        patch_sq_sum_raw += p * p;
                    }
                }

                let patch_var = patch_sq_sum_raw - (patch_sum * patch_sum) * inv_t_count;
                if patch_var > 1e-4 {
                    let patch_norm = patch_var.sqrt();
                    let ncc = cross_prod / (templ_norm * patch_norm);
                    if ncc > best_score {
                        best_score = ncc;
                        best_pos = (sx, sy);

                        if ncc >= 0.995 {
                            // Near-perfect match found early!
                            break;
                        }
                    }
                }
            }
        }

        let duration_us = start.elapsed().as_micros();
        let center_x = (best_pos.0 + template_w / 2) as i32;
        let center_y = (best_pos.1 + template_h / 2) as i32;
        let matched = best_score >= threshold;

        MatchResult {
            matched,
            x: center_x,
            y: center_y,
            confidence: best_score.max(0.0),
            duration_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radar_exact_match() {
        let sw = 100u32;
        let sh = 100u32;
        let mut search_img = vec![40u8; (sw * sh * 4) as usize];

        let tw = 20u32;
        let th = 20u32;
        let mut templ_img = vec![0u8; (tw * th * 4) as usize];

        // Fill template with distinct high-contrast pattern
        for y in 0..th {
            for x in 0..tw {
                let idx = ((y * tw + x) * 4) as usize;
                let v = if (x + y) % 2 == 0 { 240 } else { 20 };
                templ_img[idx] = v;
                templ_img[idx + 1] = v;
                templ_img[idx + 2] = v;
                templ_img[idx + 3] = 255;
            }
        }

        // Embed template into search image at (35, 45)
        let target_x = 35;
        let target_y = 45;
        for y in 0..th {
            for x in 0..tw {
                let src_idx = ((y * tw + x) * 4) as usize;
                let dst_idx = (((target_y + y) * sw + (target_x + x)) * 4) as usize;
                search_img[dst_idx..dst_idx + 4].copy_from_slice(&templ_img[src_idx..src_idx + 4]);
            }
        }

        // Search in a local ROI around expected point (+/- 30px)
        let roi = SearchRoi {
            x: 20,
            y: 30,
            width: 50,
            height: 50,
        };

        let result = RadarTracker::find_template(
            &search_img,
            sw,
            sh,
            &templ_img,
            tw,
            th,
            Some(roi),
            DEFAULT_RADAR_CONFIDENCE_THRESHOLD,
        );

        println!(
            "Radar Match: matched={}, x={}, y={}, conf={:.4}, time={}us ({}ms)",
            result.matched,
            result.x,
            result.y,
            result.confidence,
            result.duration_us,
            result.duration_us as f64 / 1000.0
        );

        assert!(result.matched);
        assert_eq!(result.x, target_x as i32 + (tw / 2) as i32);
        assert_eq!(result.y, target_y as i32 + (th / 2) as i32);
        assert!(result.confidence > 0.98);
        if !cfg!(debug_assertions) {
            assert!(result.duration_us <= 3000, "Release execution time must be <= 3ms, got {}us", result.duration_us);
        }
    }
}
