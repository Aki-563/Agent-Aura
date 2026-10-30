//! Set-of-Mark grounding: pick the most useful on-screen elements, number them, and draw the
//! numbers onto the screenshot the model sees. The model can then answer "click element 7"
//! instead of estimating pixels, which is far more reliable for every vision model.
//!
//! All geometry here is in the pixel space of the image sent to the model.

use image::{Rgba, RgbaImage};
use serde::Serialize;

use crate::perception::{CandidateSource, GroundingCandidate};

/// Elements shown to the model per step. Enough for a busy window, few enough to stay readable.
pub const MAX_MARKS: usize = 50;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    pub id: u32,
    pub label: String,
    pub kind: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Element {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

/// UI Automation control types a user would actually click or type into.
const INTERACTIVE_TYPES: [&str; 16] = [
    "Button", "Edit", "MenuItem", "TabItem", "CheckBox", "RadioButton", "ComboBox", "Hyperlink",
    "ListItem", "TreeItem", "SplitButton", "Slider", "Spinner", "DataItem", "Document", "Menu",
];

/// Window furniture a person almost never needs to click by number: scroll-bar parts and the like.
/// They are kept as a last resort, behind everything else, so they cannot crowd out real content.
fn is_chrome(label: &str, control_type: Option<&str>) -> bool {
    const CHROME_LABELS: [&str; 14] = [
        "line up", "line down", "page up", "page down", "page left", "page right", "column left", "column right",
        "thumbnail module", "filter drop-down", "view slider", "system", "horizontal scroll bar", "vertical scroll bar",
    ];
    let label = label.trim().to_lowercase();
    CHROME_LABELS.contains(&label.as_str()) || control_type.is_some_and(|kind| kind.contains("ScrollBar") || kind.contains("Thumb"))
}

/// Rows of a list, tree or table: the content a task is usually about.
fn is_row(control_type: Option<&str>) -> bool {
    control_type.is_some_and(|kind| ["ListItem", "DataItem", "TreeItem", "TabItem"].iter().any(|known| kind.contains(known)))
}

fn is_interactive(control_type: Option<&str>) -> bool {
    control_type.is_some_and(|kind| INTERACTIVE_TYPES.iter().any(|known| kind.contains(known)))
}

struct Scaled<'a> {
    candidate: &'a GroundingCandidate,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    priority: u8,
    relevance: u8,
    row: bool,
}

/// Chooses up to `max` elements from raw UIA/OCR candidates and numbers them in reading order.
///
/// * Candidates are converted from physical desktop pixels into screenshot pixels and clipped.
/// * Containers covering most of the screen, and slivers, are dropped.
/// * Interactive UIA controls win over OCR text, which wins over non-interactive UIA nodes.
/// * Elements whose label shares a word with the task come first (the task says what to look for).
/// * Slots are spread across a 4x3 grid of screen regions so one busy corner (title bar, sidebar)
///   cannot use them all; leftover slots are then filled by the remaining best candidates.
/// * OCR lines whose centre lies inside an already chosen element are dropped as duplicates.
pub fn select_elements(
    candidates: &[GroundingCandidate],
    origin: (i32, i32),
    screen: (u32, u32),
    image: (u32, u32),
    max: usize,
    task: &str,
) -> Vec<Element> {
    if screen.0 == 0 || screen.1 == 0 || image.0 == 0 || image.1 == 0 {
        return Vec::new();
    }
    let (sx, sy) = (image.0 as f64 / screen.0 as f64, image.1 as f64 / screen.1 as f64);
    let (max_x, max_y) = (image.0 as i32 - 1, image.1 as i32 - 1);
    let screen_area = (image.0 as i64) * (image.1 as i64);
    let task_words = words(task);

    let mut scaled: Vec<Scaled> = candidates
        .iter()
        .filter_map(|c| {
            let left = (((c.left - origin.0) as f64) * sx).round() as i32;
            let top = (((c.top - origin.1) as f64) * sy).round() as i32;
            let right = (((c.right - origin.0) as f64) * sx).round() as i32;
            let bottom = (((c.bottom - origin.1) as f64) * sy).round() as i32;
            if right <= 0 || bottom <= 0 || left > max_x || top > max_y {
                return None;
            }
            let (left, top) = (left.max(0), top.max(0));
            let (right, bottom) = (right.min(max_x), bottom.min(max_y));
            let (w, h) = ((right - left) as i64, (bottom - top) as i64);
            if w < 4 || h < 4 || w * h * 10 > screen_area * 6 {
                return None;
            }
            if c.label.trim().is_empty() {
                return None;
            }
            let priority = match c.source {
                _ if is_chrome(&c.label, c.control_type.as_deref()) => 3,
                CandidateSource::VisualAnchor => 0,
                CandidateSource::UiAutomation if is_interactive(c.control_type.as_deref()) => 0,
                CandidateSource::Ocr => 1,
                CandidateSource::UiAutomation => 2,
            };
            let relevance = words(&c.label).iter().filter(|word| task_words.contains(*word)).count().min(2) as u8;
            let row = is_row(c.control_type.as_deref());
            Some(Scaled { candidate: c, left, top, right, bottom, priority, relevance, row })
        })
        .collect();

    // Task-relevant first, then by kind, then smaller boxes (more specific targets).
    scaled.sort_by_key(|s| {
        (
            std::cmp::Reverse(s.relevance),
            s.priority,
            // Smaller boxes are more specific, but a wide list row is not "less specific" than a
            // button: sizes above a modest bound tie, and a row wins the tie.
            ((s.right - s.left) as i64 * (s.bottom - s.top) as i64).min(2500),
            !s.row,
        )
    });

    const GRID_COLUMNS: i32 = 4;
    const GRID_ROWS: i32 = 3;
    let cell_cap = (max * 2 / (GRID_COLUMNS * GRID_ROWS) as usize).max(3);
    let cell_of = |s: &Scaled| {
        let column = ((s.left + s.right) / 2 * GRID_COLUMNS / image.0 as i32).clamp(0, GRID_COLUMNS - 1);
        let row = ((s.top + s.bottom) / 2 * GRID_ROWS / image.1 as i32).clamp(0, GRID_ROWS - 1);
        (row * GRID_COLUMNS + column) as usize
    };
    let is_duplicate = |item: &Scaled, chosen: &[&Scaled]| {
        let center = ((item.left + item.right) / 2, (item.top + item.bottom) / 2);
        chosen.iter().any(|kept| {
            let inside = center.0 >= kept.left && center.0 <= kept.right && center.1 >= kept.top && center.1 <= kept.bottom;
            // Only a control about one line tall (a button, a field, a list row) owns the text inside
            // it. A page or pane does not: when an app exposes nothing but a bare container, its
            // words exist only as OCR lines and must stay visible.
            let line_sized = (kept.bottom - kept.top) <= 3 * (item.bottom - item.top).max(8);
            // OCR text sitting inside a chosen control, or two near-identical boxes.
            (item.priority >= 1 && inside && line_sized) || same_box(item, kept)
        })
    };

    let mut chosen: Vec<&Scaled> = Vec::new();
    let mut per_cell = vec![0usize; (GRID_COLUMNS * GRID_ROWS) as usize];
    let mut skipped: Vec<&Scaled> = Vec::new();
    // Rows of lists, trees and tables are the content a task is usually about; they get their own
    // allowance instead of competing for a busy region's slots with the toolbar around them.
    const ROW_ALLOWANCE: usize = 15;
    let mut rows_chosen = 0;
    for item in &scaled {
        if chosen.len() >= max {
            break;
        }
        if is_duplicate(item, &chosen) {
            continue;
        }
        if item.row && item.priority == 0 && rows_chosen < ROW_ALLOWANCE {
            rows_chosen += 1;
            chosen.push(item);
            continue;
        }
        let cell = cell_of(item);
        // Window furniture never claims a region's slots; it only fills what is left at the end.
        if item.priority >= 3 || per_cell[cell] >= cell_cap {
            skipped.push(item);
            continue;
        }
        per_cell[cell] += 1;
        chosen.push(item);
    }
    // Second pass: use any slots the per-region cap left free.
    for item in skipped {
        if chosen.len() >= max {
            break;
        }
        if !is_duplicate(item, &chosen) {
            chosen.push(item);
        }
    }

    // Reading order: rows of ~24 px top to bottom, then left to right.
    chosen.sort_by_key(|s| (s.top / 24, s.left));
    chosen
        .into_iter()
        .enumerate()
        .map(|(index, s)| Element {
            id: index as u32 + 1,
            label: truncate_label(&s.candidate.label),
            kind: kind_of(s.candidate),
            left: s.left,
            top: s.top,
            right: s.right,
            bottom: s.bottom,
        })
        .collect()
}

/// Lowercase alphanumeric words of at least 3 characters (stop-word-ish noise is short anyway).
fn words(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_owned)
        .collect()
}

fn same_box(a: &Scaled, b: &Scaled) -> bool {
    (a.left - b.left).abs() <= 3 && (a.top - b.top).abs() <= 3 && (a.right - b.right).abs() <= 3 && (a.bottom - b.bottom).abs() <= 3
}

fn truncate_label(label: &str) -> String {
    let single_line: String = label.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line.chars().take(60).collect()
}

fn kind_of(candidate: &GroundingCandidate) -> String {
    match candidate.source {
        CandidateSource::Ocr => "text".to_owned(),
        CandidateSource::VisualAnchor => "remembered".to_owned(),
        CandidateSource::UiAutomation => candidate
            .control_type
            .as_deref()
            .map(|k| k.to_lowercase())
            .unwrap_or_else(|| "control".to_owned()),
    }
}

/// Compact one-line-per-element text for the prompt.
pub fn element_list_text(elements: &[Element]) -> String {
    if elements.is_empty() {
        return "(none detected: use pixel coordinates)".to_owned();
    }
    elements
        .iter()
        .map(|e| format!("[{}] {} \"{}\" ({},{})-({},{})", e.id, e.kind, e.label, e.left, e.top, e.right, e.bottom))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- drawing ---------------------------------------------------------------------------------

const PALETTE: [[u8; 3]; 6] = [
    [230, 25, 75],
    [0, 130, 200],
    [245, 130, 48],
    [60, 180, 75],
    [145, 30, 180],
    [0, 150, 150],
];

/// 3x5 bitmaps for digits 0-9, one row per entry (bit 2 = left pixel).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
const GLYPH_SCALE: i32 = 2;

fn put(image: &mut RgbaImage, x: i32, y: i32, color: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
        image.put_pixel(x as u32, y as u32, Rgba([color[0], color[1], color[2], 255]));
    }
}

fn fill_rect(image: &mut RgbaImage, left: i32, top: i32, right: i32, bottom: i32, color: [u8; 3]) {
    for y in top..=bottom {
        for x in left..=right {
            put(image, x, y, color);
        }
    }
}

fn outline(image: &mut RgbaImage, e: &Element, color: [u8; 3]) {
    for t in 0..2 {
        for x in e.left..=e.right {
            put(image, x, e.top + t, color);
            put(image, x, e.bottom - t, color);
        }
        for y in e.top..=e.bottom {
            put(image, e.left + t, y, color);
            put(image, e.right - t, y, color);
        }
    }
}

fn draw_number(image: &mut RgbaImage, id: u32, left: i32, top: i32, background: [u8; 3]) {
    let text = id.to_string();
    let glyph_w = 3 * GLYPH_SCALE;
    let glyph_h = 5 * GLYPH_SCALE;
    let width = text.len() as i32 * (glyph_w + GLYPH_SCALE) + GLYPH_SCALE;
    fill_rect(image, left, top, left + width, top + glyph_h + GLYPH_SCALE * 2, background);
    for (index, ch) in text.chars().enumerate() {
        let digit = ch.to_digit(10).unwrap_or(0) as usize;
        let origin_x = left + GLYPH_SCALE + index as i32 * (glyph_w + GLYPH_SCALE);
        let origin_y = top + GLYPH_SCALE;
        for (row, bits) in DIGITS[digit].iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    fill_rect(
                        image,
                        origin_x + col * GLYPH_SCALE,
                        origin_y + row as i32 * GLYPH_SCALE,
                        origin_x + (col + 1) * GLYPH_SCALE - 1,
                        origin_y + (row as i32 + 1) * GLYPH_SCALE - 1,
                        [255, 255, 255],
                    );
                }
            }
        }
    }
}

/// Draws every element's box and number onto `image`.
pub fn draw_marks(image: &mut RgbaImage, elements: &[Element]) {
    for element in elements {
        let color = PALETTE[element.id as usize % PALETTE.len()];
        outline(image, element, color);
        // Label sits just above the box when there is room, otherwise inside its top-left corner.
        let label_height = 5 * GLYPH_SCALE + GLYPH_SCALE * 2;
        let label_top = if element.top - label_height - 1 >= 0 { element.top - label_height - 1 } else { element.top + 1 };
        draw_number(image, element.id, element.left, label_top, color);
    }
}

/// Draws a high-contrast crosshair where a pointer action is about to land.
pub fn draw_crosshair(image: &mut RgbaImage, x: i32, y: i32) {
    for offset in -14..=14 {
        for thickness in -1..=1 {
            put(image, x + offset, y + thickness, [255, 0, 0]);
            put(image, x + thickness, y + offset, [255, 0, 0]);
        }
    }
    for offset in -3i32..=3 {
        for inner in -3i32..=3 {
            if offset.abs() + inner.abs() <= 3 {
                put(image, x + offset, y + inner, [255, 255, 0]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(source: CandidateSource, label: &str, kind: Option<&str>, l: i32, t: i32, r: i32, b: i32) -> GroundingCandidate {
        GroundingCandidate {
            source,
            label: label.to_owned(),
            control_type: kind.map(str::to_owned),
            left: l,
            top: t,
            right: r,
            bottom: b,
            confidence: None,
        }
    }

    // Monitor 1920x1080 at origin 0,0; screenshot 1280x720 (scale 2/3).
    fn select(candidates: &[GroundingCandidate], max: usize) -> Vec<Element> {
        select_elements(candidates, (0, 0), (1920, 1080), (1280, 720), max, "")
    }

    #[test]
    fn text_inside_a_bare_page_container_stays_visible() {
        // A browser whose page exposes no accessibility children: UI Automation reports one big
        // document, and the page's words exist only as OCR lines inside it (seen 2026-10-30 in a
        // fresh Chrome instance: "Double-click me" vanished from the numbered elements).
        let candidates = vec![
            candidate(CandidateSource::UiAutomation, "Aura Fixture Mouse", Some("Document"), 19, 120, 1317, 1002),
            candidate(CandidateSource::Ocr, "Double-click me", None, 160, 422, 336, 443),
            candidate(CandidateSource::Ocr, "Right-click me", None, 527, 422, 682, 449),
            // A real button and its own label read by OCR: the label is still a duplicate.
            candidate(CandidateSource::UiAutomation, "Save", Some("Button"), 900, 500, 1000, 540),
            candidate(CandidateSource::Ocr, "Save", None, 930, 510, 970, 530),
        ];
        let elements = select(&candidates, MAX_MARKS);
        let labels: Vec<&str> = elements.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"Double-click me"), "page text must be numbered: {labels:?}");
        assert!(labels.contains(&"Right-click me"), "page text must be numbered: {labels:?}");
        assert_eq!(labels.iter().filter(|l| **l == "Save").count(), 1, "a button's own label is not listed twice: {labels:?}");
    }

    #[test]
    fn a_list_row_in_a_busy_region_is_not_crowded_out_by_text_elsewhere() {
        // A dialog: a dozen small controls in one region, the file row in the same region, and plenty
        // of free text across the rest of the screen that would take every leftover slot.
        let mut candidates = Vec::new();
        for i in 0..12 {
            candidates.push(candidate(CandidateSource::UiAutomation, &format!("Button {i}"), Some("Button"), 40 + i * 20, 40, 56 + i * 20, 56));
        }
        candidates.push(candidate(CandidateSource::UiAutomation, "report.txt", Some("ListItem"), 40, 90, 420, 120));
        for i in 0..60 {
            let (x, y) = (500 + (i % 6) * 220, 200 + (i / 6) * 70);
            candidates.push(candidate(CandidateSource::Ocr, &format!("free text {i}"), None, x, y, x + 150, y + 20));
        }
        let elements = select(&candidates, MAX_MARKS);
        assert!(elements.iter().any(|e| e.label == "report.txt"), "the file row must be numbered");
    }

    #[test]
    fn a_wide_list_row_is_not_crowded_out_by_window_chrome() {
        // A file dialog: many small toolbar buttons and scroll-bar parts, and the one row that matters.
        let mut candidates = Vec::new();
        for i in 0..60 {
            candidates.push(candidate(CandidateSource::UiAutomation, &format!("Tool {i}"), Some("Button"), 20 + i * 20, 20, 36 + i * 20, 36));
        }
        for i in 0..10 {
            let label = if i % 2 == 0 { "Line up" } else { "Page down" };
            candidates.push(candidate(CandidateSource::UiAutomation, label, Some("Button"), 1800, 100 + i * 20, 1816, 116 + i * 20));
        }
        candidates.push(candidate(CandidateSource::UiAutomation, "report.txt", Some("ListItem"), 400, 500, 1500, 540));
        let elements = select(&candidates, MAX_MARKS);
        assert!(elements.iter().any(|e| e.label == "report.txt"), "the file row must be numbered");
        assert!(elements.iter().filter(|e| e.label == "Line up" || e.label == "Page down").count() == 0, "chrome must come last");
    }

    #[test]
    fn converts_to_screenshot_pixels_and_numbers_in_reading_order() {
        let elements = select(
            &[
                candidate(CandidateSource::UiAutomation, "Cancel", Some("Button"), 900, 600, 1050, 660),
                candidate(CandidateSource::UiAutomation, "Save", Some("Button"), 300, 600, 450, 660),
                candidate(CandidateSource::UiAutomation, "File name", Some("Edit"), 300, 300, 900, 360),
            ],
            40,
        );
        let labels: Vec<_> = elements.iter().map(|e| (e.id, e.label.as_str())).collect();
        assert_eq!(labels, [(1, "File name"), (2, "Save"), (3, "Cancel")]);
        assert_eq!((elements[1].left, elements[1].top, elements[1].right, elements[1].bottom), (200, 400, 300, 440));
        assert_eq!(elements[1].center(), (250, 420));
    }

    #[test]
    fn drops_containers_slivers_offscreen_and_unlabelled() {
        let elements = select(
            &[
                candidate(CandidateSource::UiAutomation, "Window", Some("Window"), 0, 0, 1900, 1070),
                candidate(CandidateSource::UiAutomation, "Sliver", Some("Button"), 100, 100, 102, 300),
                candidate(CandidateSource::UiAutomation, "Elsewhere", Some("Button"), 5000, 100, 5200, 200),
                candidate(CandidateSource::UiAutomation, "  ", Some("Button"), 100, 100, 200, 160),
                candidate(CandidateSource::UiAutomation, "Real", Some("Button"), 100, 400, 220, 460),
            ],
            40,
        );
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].label, "Real");
    }

    #[test]
    fn ocr_text_inside_a_control_is_a_duplicate_but_free_text_is_kept() {
        let elements = select(
            &[
                candidate(CandidateSource::UiAutomation, "Save", Some("Button"), 300, 600, 450, 660),
                candidate(CandidateSource::Ocr, "Save", Some("text"), 330, 615, 420, 645),
                candidate(CandidateSource::Ocr, "Welcome back", Some("text"), 300, 100, 600, 140),
            ],
            40,
        );
        let labels: Vec<_> = elements.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["Welcome back", "Save"]);
    }

    #[test]
    fn interactive_controls_win_when_the_cap_is_tight() {
        let mut candidates = vec![candidate(CandidateSource::UiAutomation, "Pane label", Some("Text"), 100, 100, 300, 130)];
        candidates.push(candidate(CandidateSource::UiAutomation, "OK", Some("Button"), 600, 600, 700, 650));
        let elements = select(&candidates, 1);
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].label, "OK");
    }

    #[test]
    fn one_busy_corner_cannot_use_every_slot() {
        // 60 menu-bar buttons in the top-left plus three controls elsewhere; only 12 slots.
        let mut candidates: Vec<_> = (0..60)
            .map(|i| candidate(CandidateSource::UiAutomation, &format!("Menu {i}"), Some("Button"), 10 + (i % 6) * 40, (i / 6) * 14, 40 + (i % 6) * 40, (i / 6) * 14 + 12))
            .collect();
        candidates.push(candidate(CandidateSource::UiAutomation, "Chat input", Some("Edit"), 900, 900, 1500, 960));
        candidates.push(candidate(CandidateSource::UiAutomation, "Send", Some("Button"), 1600, 900, 1700, 960));
        candidates.push(candidate(CandidateSource::UiAutomation, "Editor tab", Some("TabItem"), 900, 100, 1100, 140));
        let elements = select(&candidates, 12);
        assert_eq!(elements.len(), 12);
        let labels: Vec<_> = elements.iter().map(|e| e.label.as_str()).collect();
        for wanted in ["Chat input", "Send", "Editor tab"] {
            assert!(labels.contains(&wanted), "{wanted} was crowded out: {labels:?}");
        }
    }

    #[test]
    fn task_words_pull_matching_elements_to_the_front() {
        let candidates: Vec<_> = (0..30)
            .map(|i| candidate(CandidateSource::UiAutomation, &format!("Item {i}"), Some("Button"), 10, i * 20, 120, i * 20 + 16))
            .chain([candidate(CandidateSource::UiAutomation, "Export report", Some("Button"), 600, 500, 760, 540)])
            .collect();
        let without = select_elements(&candidates, (0, 0), (1920, 1080), (1280, 720), 5, "");
        assert!(!without.iter().any(|e| e.label == "Export report"));
        let with = select_elements(&candidates, (0, 0), (1920, 1080), (1280, 720), 5, "please export the sales report");
        assert!(with.iter().any(|e| e.label == "Export report"));
    }

    #[test]
    fn respects_the_element_cap() {
        let candidates: Vec<_> = (0..100)
            .map(|i| candidate(CandidateSource::UiAutomation, &format!("Item {i}"), Some("ListItem"), 10, i * 10, 300, i * 10 + 9))
            .collect();
        assert_eq!(select(&candidates, MAX_MARKS).len(), MAX_MARKS);
    }

    #[test]
    fn list_text_is_compact_and_handles_empty() {
        let elements = select(&[candidate(CandidateSource::UiAutomation, "Save", Some("Button"), 300, 600, 450, 660)], 40);
        assert_eq!(element_list_text(&elements), "[1] button \"Save\" (200,400)-(300,440)");
        assert!(element_list_text(&[]).contains("none detected"));
    }

    #[test]
    fn drawing_changes_pixels_inside_the_image_and_never_panics_at_edges() {
        let mut image = RgbaImage::from_pixel(200, 100, Rgba([255, 255, 255, 255]));
        let elements = vec![
            Element { id: 7, label: "a".into(), kind: "button".into(), left: 20, top: 30, right: 80, bottom: 60 },
            Element { id: 12, label: "b".into(), kind: "button".into(), left: 150, top: 0, right: 199, bottom: 20 },
        ];
        draw_marks(&mut image, &elements);
        assert_ne!(image.get_pixel(20, 45).0, [255, 255, 255, 255], "left edge of the box is drawn");
        assert_ne!(image.get_pixel(50, 30).0, [255, 255, 255, 255], "top edge is drawn");
        draw_crosshair(&mut image, 0, 0);
        draw_crosshair(&mut image, 199, 99);
        assert_eq!(image.get_pixel(0, 0).0, [255, 255, 0, 255], "crosshair centre is yellow");
    }
}
