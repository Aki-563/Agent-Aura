//! Geometry for strokes: parametric shapes the model can ask for by name (circle, spiral, bezier,
//! ...) and the resampling that turns any list of points into one smooth, gap-free stroke.
//!
//! Pure functions, no input is sent here. `draw_path` (send_input/ext.rs) replays the result.

/// A shape the model names instead of listing every point (`draw_path` with `shape`).
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    Polyline { points: Vec<(f64, f64)> },
    Circle { cx: f64, cy: f64, r: f64 },
    Ellipse { cx: f64, cy: f64, rx: f64, ry: f64, rotation_deg: f64 },
    Spiral { cx: f64, cy: f64, r_start: f64, r_end: f64, turns: f64 },
    /// Quadratic (3 points) or cubic (4 points) bezier.
    Bezier { points: Vec<(f64, f64)> },
    /// A sine wave from (x1, y) to (x2, y).
    Sine { x1: f64, x2: f64, y: f64, amplitude: f64, wavelength: f64 },
}

impl Shape {
    /// Builds a shape from loose JSON fields; the error says exactly what is missing.
    pub fn from_json(value: &serde_json::Value) -> Result<Shape, String> {
        let num = |key: &str| value.get(key).and_then(lenient_number);
        let need = |key: &str| num(key).ok_or_else(|| format!("shape needs a number '{key}'"));
        let points = |key: &str| -> Option<Vec<(f64, f64)>> {
            value.get(key)?.as_array()?.iter().map(|p| {
                let pair = p.as_array()?;
                Some((lenient_number(pair.first()?)?, lenient_number(pair.get(1)?)?))
            }).collect()
        };
        let kind = value.get("kind").or_else(|| value.get("type")).and_then(|k| k.as_str()).unwrap_or("").trim().to_lowercase();
        Ok(match kind.as_str() {
            "line" => Shape::Line { x1: need("x1")?, y1: need("y1")?, x2: need("x2")?, y2: need("y2")? },
            "polyline" | "polygon" | "path" => Shape::Polyline { points: points("points").filter(|p| p.len() >= 2).ok_or("polyline needs 'points' [[x,y],...] with at least 2 points")? },
            "circle" => Shape::Circle { cx: need("cx")?, cy: need("cy")?, r: need("r").or_else(|_| need("radius"))? },
            "ellipse" | "oval" => Shape::Ellipse { cx: need("cx")?, cy: need("cy")?, rx: need("rx")?, ry: need("ry")?, rotation_deg: num("rotation").unwrap_or(0.0) },
            "spiral" => Shape::Spiral { cx: need("cx")?, cy: need("cy")?, r_start: num("r_start").unwrap_or(0.0), r_end: need("r_end").or_else(|_| need("r"))?, turns: num("turns").unwrap_or(3.0) },
            "bezier" | "curve" => Shape::Bezier { points: points("points").filter(|p| p.len() == 3 || p.len() == 4).ok_or("bezier needs 'points' with 3 (quadratic) or 4 (cubic) control points")? },
            "sine" | "wave" => Shape::Sine { x1: need("x1")?, x2: need("x2")?, y: need("y")?, amplitude: need("amplitude")?, wavelength: need("wavelength")? },
            "" => return Err("shape needs a 'kind': line, polyline, circle, ellipse, spiral, bezier or sine".into()),
            other => return Err(format!("unknown shape kind '{other}': use line, polyline, circle, ellipse, spiral, bezier or sine")),
        })
    }

    /// Dense outline of the shape (about one point per pixel of arc length), in drawing order.
    pub fn outline(&self) -> Vec<(f64, f64)> {
        match self {
            Shape::Line { x1, y1, x2, y2 } => vec![(*x1, *y1), (*x2, *y2)],
            Shape::Polyline { points } => points.clone(),
            Shape::Circle { cx, cy, r } => sample_closed(|t| (cx + r * t.cos(), cy + r * t.sin()), std::f64::consts::TAU * r.abs()),
            Shape::Ellipse { cx, cy, rx, ry, rotation_deg } => {
                let (s, c) = rotation_deg.to_radians().sin_cos();
                sample_closed(
                    |t| {
                        let (ex, ey) = (rx * t.cos(), ry * t.sin());
                        (cx + ex * c - ey * s, cy + ex * s + ey * c)
                    },
                    std::f64::consts::TAU * rx.abs().max(ry.abs()),
                )
            }
            Shape::Spiral { cx, cy, r_start, r_end, turns } => {
                let total_angle = std::f64::consts::TAU * turns.max(0.1);
                let length = total_angle * (r_start.abs() + r_end.abs()) / 2.0;
                let n = (length.ceil() as usize).clamp(16, 20_000);
                (0..=n)
                    .map(|i| {
                        let t = i as f64 / n as f64;
                        let a = t * total_angle;
                        let r = r_start + (r_end - r_start) * t;
                        (cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect()
            }
            Shape::Bezier { points } => {
                let rough: f64 = points.windows(2).map(|w| dist(w[0], w[1])).sum();
                let n = (rough.ceil() as usize).clamp(16, 20_000);
                (0..=n).map(|i| bezier_point(points, i as f64 / n as f64)).collect()
            }
            Shape::Sine { x1, x2, y, amplitude, wavelength } => {
                let span = x2 - x1;
                let n = ((span.abs() * 2.0).ceil() as usize).clamp(16, 20_000);
                let wavelength = if wavelength.abs() < 1.0 { 1.0 } else { *wavelength };
                (0..=n)
                    .map(|i| {
                        let x = x1 + span * i as f64 / n as f64;
                        (x, y + amplitude * (std::f64::consts::TAU * (x - x1) / wavelength).sin())
                    })
                    .collect()
            }
        }
    }

    /// Whether the outline should end where it started.
    pub fn is_closed(&self) -> bool {
        matches!(self, Shape::Circle { .. } | Shape::Ellipse { .. })
    }
}

/// Accepts 12, 12.5 and "12" (models sometimes quote numbers).
pub fn lenient_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn sample_closed(at: impl Fn(f64) -> (f64, f64), perimeter: f64) -> Vec<(f64, f64)> {
    let n = (perimeter.ceil() as usize).clamp(16, 20_000);
    (0..=n).map(|i| at(std::f64::consts::TAU * i as f64 / n as f64)).collect()
}

fn bezier_point(p: &[(f64, f64)], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    if p.len() == 3 {
        let (a, b, c) = (u * u, 2.0 * u * t, t * t);
        (a * p[0].0 + b * p[1].0 + c * p[2].0, a * p[0].1 + b * p[1].1 + c * p[2].1)
    } else {
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        (a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0, a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1)
    }
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Integer points along the polyline through `points`, consecutive points at most one pixel apart
/// in each direction (a stroke with no gaps), exact duplicates removed.
pub fn densify(points: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        if i == 0 {
            out.push(p);
            continue;
        }
        let a = points[i - 1];
        let steps = (p.0 - a.0).abs().max((p.1 - a.1).abs());
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            let q = ((a.0 as f64 + (p.0 - a.0) as f64 * t).round() as i32, (a.1 as f64 + (p.1 - a.1) as f64 * t).round() as i32);
            if out.last() != Some(&q) {
                out.push(q);
            }
        }
        if steps == 0 && out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// Rounds a float outline to integer pixels, dropping repeats.
pub fn to_pixels(points: &[(f64, f64)]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for &(x, y) in points {
        let q = (x.round() as i32, y.round() as i32);
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    out
}

/// Picks the points actually sent as events: every `stride`-th point of the dense stroke, always
/// keeping the first, the last and every sharp corner (so a polygon's corners are not cut).
pub fn event_points(dense: &[(i32, i32)], stride: usize) -> Vec<(i32, i32)> {
    let stride = stride.max(1);
    let n = dense.len();
    let mut out = Vec::with_capacity(n / stride + 2);
    for (i, &p) in dense.iter().enumerate() {
        if i == 0 || i + 1 == n || i % stride == 0 || is_corner(dense, i) {
            out.push(p);
        }
    }
    out
}

/// A turn of more than about 30 degrees, measured over 3 pixels on each side so the one-pixel
/// staircase of a smooth diagonal does not count as a corner.
fn is_corner(dense: &[(i32, i32)], i: usize) -> bool {
    const SPAN: usize = 3;
    if i < SPAN || i + SPAN >= dense.len() {
        return false;
    }
    let (a, b, c) = (dense[i - SPAN], dense[i], dense[i + SPAN]);
    let (ux, uy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    let (vx, vy) = ((c.0 - b.0) as f64, (c.1 - b.1) as f64);
    let (lu, lv) = (ux.hypot(uy), vx.hypot(vy));
    lu > 0.0 && lv > 0.0 && (ux * vx + uy * vy) / (lu * lv) < 0.86
}

/// Largest distance from any point of `drawn` to the nearest point of `ideal` (both dense).
pub fn max_deviation(drawn: &[(f64, f64)], ideal: &[(f64, f64)]) -> f64 {
    drawn
        .iter()
        .map(|d| ideal.iter().map(|i| dist(*d, *i)).fold(f64::INFINITY, f64::min))
        .fold(0.0, f64::max)
}

/// Largest gap between consecutive points.
pub fn largest_gap(points: &[(i32, i32)]) -> f64 {
    points.windows(2).map(|w| dist((w[0].0 as f64, w[0].1 as f64), (w[1].0 as f64, w[1].1 as f64))).fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn as_f64(points: &[(i32, i32)]) -> Vec<(f64, f64)> {
        points.iter().map(|&(x, y)| (x as f64, y as f64)).collect()
    }

    #[test]
    fn a_circle_is_a_closed_smooth_stroke_within_one_pixel() {
        let shape = Shape::from_json(&json!({"kind": "circle", "cx": 500, "cy": 400, "r": 150})).unwrap();
        let outline = shape.outline();
        let dense = densify(&to_pixels(&outline));
        assert!(largest_gap(&dense) <= 1.5, "no gaps: {}", largest_gap(&dense));
        assert!(max_deviation(&as_f64(&dense), &outline) <= 1.0, "deviation {}", max_deviation(&as_f64(&dense), &outline));
        assert_eq!(dense.first(), dense.last(), "a circle ends where it starts");
        assert!(shape.is_closed());
    }

    #[test]
    fn spiral_bezier_sine_and_ellipse_stay_on_their_ideal_curve() {
        for spec in [
            json!({"kind": "spiral", "cx": 300, "cy": 300, "r_start": 5, "r_end": 120, "turns": 4}),
            json!({"kind": "bezier", "points": [[10, 10], [200, 400], [400, -50], [600, 300]]}),
            json!({"kind": "bezier", "points": [[10, 10], [200, 400], [400, 10]]}),
            json!({"kind": "sine", "x1": 50, "x2": 650, "y": 300, "amplitude": 60, "wavelength": 150}),
            json!({"kind": "ellipse", "cx": 400, "cy": 300, "rx": 200, "ry": 80, "rotation": 30}),
            json!({"kind": "line", "x1": 0, "y1": 0, "x2": 333, "y2": 101}),
        ] {
            let shape = Shape::from_json(&spec).unwrap();
            let outline = shape.outline();
            let dense = densify(&to_pixels(&outline));
            assert!(largest_gap(&dense) <= 1.5, "{spec}: gap {}", largest_gap(&dense));
            let deviation = max_deviation(&as_f64(&dense), &densify_f(&outline));
            assert!(deviation <= 1.0, "{spec}: deviation {deviation}");
        }
    }

    fn densify_f(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        // A finer ideal (0.25 px) so the measured deviation is the stroke's, not the reference's.
        let mut out = Vec::new();
        for w in points.windows(2) {
            let n = (dist(w[0], w[1]) * 4.0).ceil().max(1.0) as usize;
            for i in 0..n {
                let t = i as f64 / n as f64;
                out.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            }
        }
        out.extend(points.last());
        out
    }

    #[test]
    fn densify_fills_every_pixel_between_far_points() {
        let dense = densify(&[(0, 0), (10, 0), (10, 5)]);
        assert_eq!(dense.len(), 16);
        assert_eq!(largest_gap(&dense), 1.0);
        assert_eq!(densify(&[(3, 3), (3, 3)]), vec![(3, 3)]);
    }

    #[test]
    fn event_points_keep_ends_and_square_corners() {
        let dense = densify(&[(0, 0), (40, 0), (40, 40), (0, 40), (0, 0)]);
        let events = event_points(&dense, 7);
        for corner in [(0, 0), (40, 0), (40, 40), (0, 40)] {
            assert!(events.contains(&corner), "corner {corner:?} kept");
        }
        assert_eq!(events.last(), Some(&(0, 0)));
        assert!(events.len() < dense.len() / 3);
    }

    #[test]
    fn shapes_explain_what_is_missing() {
        assert!(Shape::from_json(&json!({"kind": "circle", "cx": 1})).unwrap_err().contains("cy"));
        assert!(Shape::from_json(&json!({"kind": "hexagon"})).unwrap_err().contains("unknown shape"));
        assert!(Shape::from_json(&json!({"kind": "bezier", "points": [[0, 0], [1, 1]]})).unwrap_err().contains("3"));
        // Quoted numbers are accepted.
        assert!(Shape::from_json(&json!({"kind": "circle", "cx": "10", "cy": "20", "radius": "5"})).is_ok());
    }
}
