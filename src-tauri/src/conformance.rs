//! Harness conformance suite: checks that every input the model can order is executed exactly,
//! with no model involved. A test page (see `eval_fixture`) reports what it received (clicks,
//! keys, text, scroll offsets, drags), and each case orders one action variation through the
//! same executor the agent uses and compares.
//!
//! It drives the real desktop through the default browser, so run it only when the screen is free.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::desktop::target as desktop_target;
use crate::eval_fixture as fx;
use crate::input::clipboard::ClipboardBridge;
use crate::input::{ActionPrimitive as A, InputActuator, MouseButton};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseResult {
    pub group: String,
    pub name: String,
    pub passed: bool,
    pub millis: u128,
    pub detail: String,
}

type CaseFn = fn(&Lab) -> Result<(), String>;

/// Page geometry: converts a lab element's CSS rectangle into physical screen pixels.
pub struct Lab {
    off_x: f64,
    off_y: f64,
    dpr: f64,
}

const LAB_URL: &str = "http://127.0.0.1:47831/lab";

fn wait_until(timeout_ms: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn act(action: A) -> Result<(), String> {
    InputActuator::execute_action(&action).map_err(|error| error.to_string())
}

fn left_click(x: i32, y: i32, clicks: u32, hold: &[&str]) -> A {
    A::Click { x, y, button: MouseButton::Left, clicks, hold_keys: hold.iter().map(|k| (*k).to_owned()).collect() }
}

fn press(key: &str) -> Result<(), String> {
    act(A::KeyPress { key: key.to_owned() })
}

fn hotkey(keys: &[&str]) -> Result<(), String> {
    act(A::Hotkey { keys: keys.iter().map(|k| (*k).to_owned()).collect() })
}

fn type_text(text: &str) -> Result<(), String> {
    act(A::TypeText { text: text.to_owned(), press_enter: false, overwrite: false })
}

fn expect_event(name: &str) -> Result<(), String> {
    if wait_until(2500, || fx::saw(name)) {
        Ok(())
    } else {
        Err(format!("page never reported '{name}' (saw: {})", fx::events().join(", ")))
    }
}

fn expect_no_event(name: &str) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(400));
    if fx::saw(name) {
        Err(format!("page reported '{name}' but should not have"))
    } else {
        Ok(())
    }
}

fn expect_state(key: &str, expected: &str) -> Result<(), String> {
    if wait_until(2500, || fx::state(key).as_deref() == Some(expected)) {
        Ok(())
    } else {
        Err(format!("state '{key}' is {:?}, expected {:?}", fx::state(key), expected))
    }
}

/// Waits for a number that has stopped changing (smooth scrolling animates for a moment).
fn stable_number(key: &str, what: &str, ok: impl Fn(f64) -> bool) -> Result<f64, String> {
    let read = || fx::state(key).and_then(|v| v.trim().parse::<f64>().ok());
    let deadline = Instant::now() + Duration::from_millis(5000);
    let mut last = read();
    let mut since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(60));
        let now = read();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= Duration::from_millis(500) && now.is_some_and(&ok) {
            return Ok(now.unwrap_or_default());
        }
        if Instant::now() >= deadline {
            return Err(format!("{key} is {last:?}, expected {what}"));
        }
    }
}

fn expect_number(key: &str, what: &str, ok: impl Fn(f64) -> bool) -> Result<f64, String> {
    let read = || fx::state(key).and_then(|v| v.trim().parse::<f64>().ok());
    if wait_until(3000, || read().is_some_and(&ok)) {
        Ok(read().unwrap_or_default())
    } else {
        Err(format!("{key} is {:?}, expected {what}", fx::state(key)))
    }
}

impl Lab {
    /// Physical screen position of a point inside an element (fractions of its width/height).
    fn at(&self, id: &str, fx_: f64, fy: f64) -> Result<(i32, i32), String> {
        let [left, top, width, height] = self.rect_in_view(id)?;
        let x = self.off_x + (left + width * fx_) * self.dpr;
        let y = self.off_y + (top + height * fy) * self.dpr;
        Ok((x.round() as i32, y.round() as i32))
    }

    /// The element's rectangle, scrolling the page first when it is not fully on screen.
    fn rect_in_view(&self, id: &str) -> Result<[f64; 4], String> {
        let mut rect = fx::rect(id).ok_or_else(|| format!("lab element '{id}' has no position yet"))?;
        if let Some(view) = fx::rect("win") {
            let visible_height = view[1];
            if rect[1] < 0.0 || rect[1] + rect[3] > visible_height {
                self.command(&format!("into:{id}"))?;
                rect = fx::rect(id).ok_or_else(|| format!("lab element '{id}' vanished"))?;
            }
        }
        Ok(rect)
    }

    fn center(&self, id: &str) -> Result<(i32, i32), String> {
        self.at(id, 0.5, 0.5)
    }

    fn click(&self, id: &str) -> Result<(), String> {
        let (x, y) = self.center(id)?;
        act(left_click(x, y, 1, &[]))
    }

    fn command(&self, text: &str) -> Result<(), String> {
        let number = fx::send_command(text);
        if wait_until(4000, || fx::state("ack").and_then(|v| v.parse::<u64>().ok()).is_some_and(|ack| ack >= number)) {
            std::thread::sleep(Duration::from_millis(350)); // let fresh element positions arrive
            Ok(())
        } else {
            Err(format!("the lab page did not acknowledge '{text}'"))
        }
    }

    /// Clean slate: page state, recorded events, window focus, nothing held down.
    fn reset(&self) -> Result<(), String> {
        InputActuator::release_all();
        if let Some(window) = desktop_target::find_window_by_name("Aura Fixture") {
            desktop_target::bring_to_front(window);
        }
        self.command("reset")?;
        // A neutral click on the page title gives the page keyboard focus.
        self.click("title")?;
        std::thread::sleep(Duration::from_millis(150));
        fx::clear();
        Ok(())
    }

}

// ----------------------------------------------------------------------------- clicking

fn click_left(lab: &Lab) -> Result<(), String> {
    lab.click("btn-a")?;
    expect_event("click:btn-a")
}

fn click_tiny_target(lab: &Lab) -> Result<(), String> {
    lab.click("tiny")?;
    expect_event("click:tiny")
}

fn click_sequence_in_order(lab: &Lab) -> Result<(), String> {
    for id in ["btn-c", "btn-a", "btn-b"] {
        lab.click(id)?;
    }
    expect_event("click:btn-b")?;
    let order: Vec<String> = fx::events().into_iter().filter(|e| e.starts_with("click:btn")).collect();
    if order == ["click:btn-c", "click:btn-a", "click:btn-b"] {
        Ok(())
    } else {
        Err(format!("clicks arrived as {order:?}"))
    }
}

fn click_each_corner_of_target(lab: &Lab) -> Result<(), String> {
    // Near all four corners of a 70-pixel-high box: the whole target must be live.
    for (index, (fx_, fy)) in [(0.08, 0.12), (0.92, 0.12), (0.08, 0.88), (0.92, 0.88)].into_iter().enumerate() {
        let (x, y) = lab.at("box", fx_, fy)?;
        act(left_click(x, y, 1, &[]))?;
        let want = index + 1;
        if !wait_until(2000, || fx::events().iter().filter(|e| e.starts_with("click:box:d")).count() >= want) {
            return Err(format!("corner {index} did not register"));
        }
        std::thread::sleep(Duration::from_millis(450)); // keep clicks from merging into a double-click
    }
    Ok(())
}

fn double_click_action(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(A::DoubleClick { x, y, button: MouseButton::Left })?;
    expect_event("dblclick:box")
}

fn double_click_via_click_count(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(left_click(x, y, 2, &[]))?;
    expect_event("dblclick:box")?;
    expect_event("click:box:d2")
}

fn triple_click_selects_paragraph(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.at("para", 0.3, 0.5)?;
    act(left_click(x, y, 3, &[]))?;
    if wait_until(2500, || fx::state("selection").is_some_and(|s| s.contains("quick brown fox") && s.contains("lazy dog"))) {
        Ok(())
    } else {
        Err(format!("selection is {:?}", fx::state("selection")))
    }
}

fn right_click(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(A::Click { x, y, button: MouseButton::Right, clicks: 1, hold_keys: vec![] })?;
    expect_event("contextmenu:box")
}

fn middle_click(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(A::Click { x, y, button: MouseButton::Middle, clicks: 1, hold_keys: vec![] })?;
    expect_event("auxclick:box:middle")
}

fn shift_click(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(left_click(x, y, 1, &["shift"]))?;
    expect_event("click:box:shift")
}

fn ctrl_click(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(left_click(x, y, 1, &["ctrl"]))?;
    expect_event("click:box:ctrl")
}

fn modifier_released_after_click(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("box")?;
    act(left_click(x, y, 1, &["shift", "ctrl"]))?;
    expect_event("click:box:shift")?;
    std::thread::sleep(Duration::from_millis(450));
    fx::clear();
    lab.click("box")?;
    expect_event("click:box:d1")?;
    expect_no_event("click:box:shift")?;
    expect_no_event("click:box:ctrl")
}

fn hover_enter_and_leave(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("hov")?;
    act(A::MouseMove { x, y })?;
    expect_event("hover:enter")?;
    let (ox, oy) = lab.center("title")?;
    act(A::MouseMove { x: ox, y: oy })?;
    expect_event("hover:leave")
}

fn click_off_screen_is_refused(_lab: &Lab) -> Result<(), String> {
    for (x, y) in [(-5, 100), (100, -5), (200_000, 100), (100, 200_000)] {
        if act(left_click(x, y, 1, &[])).is_ok() {
            return Err(format!("click at ({x}, {y}) was not refused"));
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------- scrolling

fn scroll_page_down_and_up(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("para")?;
    act(A::Scroll { clicks: -5, x: Some(x), y: Some(y), horizontal: false })?;
    let down = stable_number("scrollY", "more than 200", |v| v > 200.0)?;
    act(A::Scroll { clicks: 5, x: Some(x), y: Some(y), horizontal: false })?;
    stable_number("scrollY", "back near the top", |v| v < down - 150.0 && v < 120.0).map(|_| ())
}

fn scroll_to_the_bottom_with_a_big_count(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("para")?;
    act(A::Scroll { clicks: -60, x: Some(x), y: Some(y), horizontal: false })?;
    let max: f64 = fx::state("scrollMax").and_then(|v| v.parse().ok()).unwrap_or(1.0e9);
    stable_number("scrollY", "the very bottom", |v| v >= max - 6.0).map(|_| ())
}

fn scroll_without_coordinates(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("para")?;
    act(A::MouseMove { x, y })?;
    act(A::Scroll { clicks: -4, x: None, y: None, horizontal: false })?;
    stable_number("scrollY", "more than 150", |v| v > 150.0).map(|_| ())
}

fn scroll_a_single_notch(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("para")?;
    act(A::Scroll { clicks: -1, x: Some(x), y: Some(y), horizontal: false })?;
    stable_number("scrollY", "between 20 and 200", |v| (20.0..=200.0).contains(&v)).map(|_| ())
}

fn scroll_inside_a_panel_not_the_page(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("inner")?;
    act(A::Scroll { clicks: -3, x: Some(x), y: Some(y), horizontal: false })?;
    stable_number("innerTop", "more than 50", |v| v > 50.0)?;
    match fx::state("scrollY").and_then(|v| v.parse::<f64>().ok()) {
        Some(page) if page > 5.0 => Err(format!("the page scrolled too (scrollY {page})")),
        _ => Ok(()),
    }
}

fn scroll_sideways(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("wide")?;
    act(A::Scroll { clicks: 4, x: Some(x), y: Some(y), horizontal: true })?;
    stable_number("innerLeft", "more than 100", |v| v > 100.0).map(|_| ())
}

fn scroll_then_click_what_appeared(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("para")?;
    act(A::Scroll { clicks: -60, x: Some(x), y: Some(y), horizontal: false })?;
    let max: f64 = fx::state("scrollMax").and_then(|v| v.parse().ok()).unwrap_or(1.0e9);
    stable_number("scrollY", "the very bottom", |v| v >= max - 6.0)?;
    std::thread::sleep(Duration::from_millis(500)); // fresh positions after the scroll
    lab.click("bottom")?;
    expect_event("click:bottom")
}

// ----------------------------------------------------------------------------- typing

fn focus_field(lab: &Lab, id: &str) -> Result<(), String> {
    lab.click(id)?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn type_short_text(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("hello world")?;
    expect_state("i1", "hello world")
}

fn type_long_text_by_paste(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    let text = "The quick brown fox jumps over the lazy dog 0123456789";
    type_text(text)?;
    expect_state("i1", text)
}

fn long_paste_keeps_the_clipboard(lab: &Lab) -> Result<(), String> {
    ClipboardBridge::set_text("AURA-CLIP-KEEP")?;
    focus_field(lab, "i1")?;
    type_text("a sentence that is long enough to be pasted through the clipboard")?;
    expect_state("i1", "a sentence that is long enough to be pasted through the clipboard")?;
    if wait_until(2000, || ClipboardBridge::get_text().as_deref() == Ok("AURA-CLIP-KEEP")) {
        Ok(())
    } else {
        Err(format!("the clipboard now holds {:?}", ClipboardBridge::get_text()))
    }
}

fn type_accents_cjk_emoji(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("café naïve 日本語 😀")?;
    expect_state("i1", "café naïve 日本語 😀")
}

fn type_every_symbol(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    // Two chunks of 20 characters or fewer, so both go through key-by-key typing.
    type_text("!@#$%^&*()_+-=[]{}")?;
    type_text(";':\",./<>?\\|~`")?;
    expect_state("i1", "!@#$%^&*()_+-=[]{};':\",./<>?\\|~`")
}

fn type_newlines_into_a_text_area(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "area")?;
    type_text("ab\ncd\r\nef")?;
    expect_state("area", "ab\ncd\nef")
}

fn type_long_multiline_by_paste(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "area")?;
    let text = "first line of a longer block\nsecond line of a longer block\nthird line";
    type_text(text)?;
    if wait_until(2500, || fx::state("area").is_some_and(|v| v.replace("\r\n", "\n") == text)) {
        Ok(())
    } else {
        Err(format!("area holds {:?}", fx::state("area")))
    }
}

fn tab_character_moves_to_the_next_field(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("one\ttwo")?;
    expect_state("i1", "one")?;
    expect_state("i2", "two")
}

fn overwrite_replaces_existing_text(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("first value")?;
    expect_state("i1", "first value")?;
    act(A::TypeText { text: "second".into(), press_enter: false, overwrite: true })?;
    expect_state("i1", "second")
}

fn type_then_press_enter_submits(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    act(A::TypeText { text: "go".into(), press_enter: true, overwrite: false })?;
    expect_state("i1", "go")?;
    expect_event("submit:form")
}

fn type_into_the_second_field_after_the_first(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("alpha")?;
    focus_field(lab, "i2")?;
    type_text("beta")?;
    expect_state("i1", "alpha")?;
    expect_state("i2", "beta")
}

fn type_a_hundred_characters_exactly(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    let text: String = (0..100).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    type_text(&text)?;
    expect_state("i1", &text)
}

// ----------------------------------------------------------------------------- keys

fn key_event(key: &str, expected: &str) -> Result<(), String> {
    press(key)?;
    expect_event(expected)
}

fn named_keys(_lab: &Lab) -> Result<(), String> {
    for (key, expected) in [
        ("enter", "key:Enter"),
        ("tab", "key:Tab"),
        ("escape", "key:Escape"),
        ("esc", "key:Escape"),
        ("backspace", "key:Backspace"),
        ("delete", "key:Delete"),
        ("f2", "key:F2"),
        ("f9", "key:F9"),
        ("space", "key:"),
    ] {
        key_event(key, expected).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

fn arrow_and_navigation_keys(_lab: &Lab) -> Result<(), String> {
    for (key, expected) in [
        ("up", "key:ArrowUp"),
        ("down", "key:ArrowDown"),
        ("left", "key:ArrowLeft"),
        ("right", "key:ArrowRight"),
        ("arrowdown", "key:ArrowDown"),
        ("home", "key:Home"),
        ("end", "key:End"),
        ("pageup", "key:PageUp"),
        ("pagedown", "key:PageDown"),
        ("insert", "key:Insert"),
    ] {
        key_event(key, expected).map_err(|e| format!("{key}: {e}"))?;
    }
    Ok(())
}

fn letters_digits_and_shifted_characters(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    for key in ["a", "Z", "5", "!", "?", "@"] {
        press(key)?;
    }
    expect_state("i1", "aZ5!?@")
}

fn chords_with_modifiers(_lab: &Lab) -> Result<(), String> {
    hotkey(&["ctrl", "shift", "f9"])?;
    expect_event("key:ctrl+shift+F9")?;
    hotkey(&["ctrl", "alt", "f9"])?;
    expect_event("key:ctrl+alt+F9")
}

fn select_all_then_type_replaces(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("old text")?;
    expect_state("i1", "old text")?;
    hotkey(&["ctrl", "a"])?;
    type_text("new")?;
    expect_state("i1", "new")
}

fn copy_and_paste_between_fields(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("carry me")?;
    hotkey(&["ctrl", "a"])?;
    hotkey(&["ctrl", "c"])?;
    focus_field(lab, "i2")?;
    hotkey(&["ctrl", "v"])?;
    expect_state("i2", "carry me")
}

fn shift_arrows_extend_selection(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("abcdef")?;
    for _ in 0..3 {
        hotkey(&["shift", "left"])?;
    }
    expect_state("sel", "def")
}

fn backspace_deletes_the_last_character(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    type_text("abc")?;
    press("backspace")?;
    expect_state("i1", "ab")
}

fn no_key_left_pressed(lab: &Lab) -> Result<(), String> {
    focus_field(lab, "i1")?;
    hotkey(&["ctrl", "shift", "f9"])?;
    std::thread::sleep(Duration::from_millis(300));
    fx::clear();
    type_text("plain")?;
    expect_state("i1", "plain")?;
    if fx::events().iter().any(|e| e.contains("ctrl+") || e.contains("shift+")) {
        Err(format!("a modifier stayed down: {:?}", fx::events()))
    } else {
        Ok(())
    }
}

// ----------------------------------------------------------------------------- dragging

fn drag_a_square_into_a_box(lab: &Lab) -> Result<(), String> {
    let (fx_, fy) = lab.center("src")?;
    let (tx, ty) = lab.center("dst")?;
    act(A::DragAndDrop { from_x: fx_, from_y: fy, to_x: tx, to_y: ty, duration_ms: 400, hold_keys: vec![] })?;
    expect_event("drop:ok")
}

fn drag_outside_the_box_misses(lab: &Lab) -> Result<(), String> {
    let (fx_, fy) = lab.center("src")?;
    let (tx, ty) = lab.center("dst")?;
    act(A::DragAndDrop { from_x: fx_, from_y: fy, to_x: tx, to_y: ty + 260, duration_ms: 400, hold_keys: vec![] })?;
    expect_event("drop:miss")
}

fn drag_a_slider(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("thumb")?;
    act(A::DragAndDrop { from_x: x, from_y: y, to_x: x + (150.0 * lab.dpr) as i32, to_y: y, duration_ms: 400, hold_keys: vec![] })?;
    expect_number("slider", "about 150", |v| (130.0..=170.0).contains(&v)).map(|_| ())
}

fn drag_a_tiny_distance(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("thumb")?;
    act(A::DragAndDrop { from_x: x, from_y: y, to_x: x + (20.0 * lab.dpr) as i32, to_y: y, duration_ms: 250, hold_keys: vec![] })?;
    expect_number("slider", "about 20", |v| (10.0..=30.0).contains(&v)).map(|_| ())
}

fn drag_right_to_left(lab: &Lab) -> Result<(), String> {
    let (x, y) = lab.center("thumb")?;
    act(A::DragAndDrop { from_x: x, from_y: y, to_x: x + (200.0 * lab.dpr) as i32, to_y: y, duration_ms: 300, hold_keys: vec![] })?;
    expect_number("slider", "about 200", |v| v >= 170.0)?;
    let (x2, y2) = lab.center("thumb")?;
    act(A::DragAndDrop { from_x: x2, from_y: y2, to_x: x2 - (120.0 * lab.dpr) as i32, to_y: y2, duration_ms: 300, hold_keys: vec![] })?;
    expect_number("slider", "about 80", |v| (60.0..=100.0).contains(&v)).map(|_| ())
}

fn drag_to_select_text(lab: &Lab) -> Result<(), String> {
    let (sx, sy) = lab.at("para", 0.01, 0.5)?;
    let (ex, ey) = lab.at("para", 0.45, 0.5)?;
    act(A::DragAndDrop { from_x: sx, from_y: sy, to_x: ex, to_y: ey, duration_ms: 400, hold_keys: vec![] })?;
    if wait_until(2500, || fx::state("selection").is_some_and(|s| s.len() > 8 && s.len() < 40 && s.starts_with("The"))) {
        Ok(())
    } else {
        Err(format!("selection is {:?}", fx::state("selection")))
    }
}

fn drag_with_the_browsers_own_drag_and_drop(lab: &Lab) -> Result<(), String> {
    let (fx_, fy) = lab.center("hsrc")?;
    let (tx, ty) = lab.center("hdst")?;
    act(A::DragAndDrop { from_x: fx_, from_y: fy, to_x: tx, to_y: ty, duration_ms: 500, hold_keys: vec![] })?;
    expect_event("html5drop:ok")
}

// ----------------------------------------------------------------------------- windows

fn focus_window_by_title(_lab: &Lab) -> Result<(), String> {
    act(A::FocusWindow { name: "Aura Fixture".into() })?;
    if wait_until(2000, || desktop_target::foreground_title().contains("Aura Fixture")) {
        Ok(())
    } else {
        Err(format!("front window is '{}'", desktop_target::foreground_title()))
    }
}

fn focus_window_unknown_is_a_clear_error(_lab: &Lab) -> Result<(), String> {
    match act(A::FocusWindow { name: "no such window zzz".into() }) {
        Err(message) if message.contains("No open window") => Ok(()),
        other => Err(format!("expected a 'No open window' error, got {other:?}")),
    }
}

fn open_url_reuses_the_current_tab(_lab: &Lab) -> Result<(), String> {
    act(A::OpenUrl { url: "http://127.0.0.1:47831/links/pricing".into() })?;
    expect_event("visit:pricing")?;
    // Back to the lab in the same tab for the remaining cases.
    fx::clear_rects();
    act(A::OpenUrl { url: LAB_URL.into() })?;
    if !wait_until(15_000, || fx::rect("btn-a").is_some()) {
        return Err("the lab page did not come back".into());
    }
    Ok(())
}

fn open_url_refuses_other_schemes(_lab: &Lab) -> Result<(), String> {
    for url in ["file:///C:/Windows/win.ini", "javascript:alert(1)", "calc.exe", "http://exa mple.com"] {
        if act(A::OpenUrl { url: url.into() }).is_ok() {
            return Err(format!("{url} was not refused"));
        }
    }
    Ok(())
}

fn wait_action_takes_the_time_asked(_lab: &Lab) -> Result<(), String> {
    let started = Instant::now();
    act(A::WaitSettle { max_timeout_ms: 600, expected_cursor: None })?;
    let took = started.elapsed().as_millis();
    if (550..1500).contains(&took) {
        Ok(())
    } else {
        Err(format!("a 600 ms wait took {took} ms"))
    }
}

fn cases() -> Vec<(&'static str, &'static str, CaseFn)> {
    vec![
        ("click", "left click", click_left),
        ("click", "tiny 22px target", click_tiny_target),
        ("click", "three clicks arrive in order", click_sequence_in_order),
        ("click", "all four corners of a target", click_each_corner_of_target),
        ("click", "double_click action", double_click_action),
        ("click", "click with clicks=2", double_click_via_click_count),
        ("click", "triple click selects the paragraph", triple_click_selects_paragraph),
        ("click", "right click", right_click),
        ("click", "middle click", middle_click),
        ("click", "shift+click", shift_click),
        ("click", "ctrl+click", ctrl_click),
        ("click", "modifiers are released after a click", modifier_released_after_click),
        ("click", "hover enters and leaves", hover_enter_and_leave),
        ("click", "off-screen clicks are refused", click_off_screen_is_refused),
        ("scroll", "page down then up", scroll_page_down_and_up),
        ("scroll", "single notch", scroll_a_single_notch),
        ("scroll", "60 notches reach the bottom", scroll_to_the_bottom_with_a_big_count),
        ("scroll", "without coordinates", scroll_without_coordinates),
        ("scroll", "inside a panel, not the page", scroll_inside_a_panel_not_the_page),
        ("scroll", "sideways", scroll_sideways),
        ("scroll", "scroll then click what appeared", scroll_then_click_what_appeared),
        ("type", "short text", type_short_text),
        ("type", "long text by paste", type_long_text_by_paste),
        ("type", "paste keeps the clipboard", long_paste_keeps_the_clipboard),
        ("type", "accents, CJK, emoji", type_accents_cjk_emoji),
        ("type", "every symbol", type_every_symbol),
        ("type", "newlines in a text area", type_newlines_into_a_text_area),
        ("type", "long multi-line paste", type_long_multiline_by_paste),
        ("type", "tab character moves to the next field", tab_character_moves_to_the_next_field),
        ("type", "overwrite replaces text", overwrite_replaces_existing_text),
        ("type", "press_enter submits", type_then_press_enter_submits),
        ("type", "two fields in a row", type_into_the_second_field_after_the_first),
        ("type", "100 characters exactly", type_a_hundred_characters_exactly),
        ("key", "named keys", named_keys),
        ("key", "arrows and navigation keys", arrow_and_navigation_keys),
        ("key", "letters, digits, shifted characters", letters_digits_and_shifted_characters),
        ("key", "chords with modifiers", chords_with_modifiers),
        ("key", "ctrl+a then type replaces", select_all_then_type_replaces),
        ("key", "copy and paste between fields", copy_and_paste_between_fields),
        ("key", "shift+arrows extend selection", shift_arrows_extend_selection),
        ("key", "backspace", backspace_deletes_the_last_character),
        ("key", "no modifier stays down", no_key_left_pressed),
        ("drag", "square into a box", drag_a_square_into_a_box),
        ("drag", "dropped outside misses", drag_outside_the_box_misses),
        ("drag", "slider", drag_a_slider),
        ("drag", "20 pixel drag", drag_a_tiny_distance),
        ("drag", "right then back left", drag_right_to_left),
        ("drag", "select text by dragging", drag_to_select_text),
        ("drag", "browser drag-and-drop", drag_with_the_browsers_own_drag_and_drop),
        ("window", "focus_window by title", focus_window_by_title),
        ("window", "unknown window is a clear error", focus_window_unknown_is_a_clear_error),
        ("window", "open_url refuses other schemes", open_url_refuses_other_schemes),
        ("window", "wait takes the time asked", wait_action_takes_the_time_asked),
        ("window", "open_url reuses the current tab", open_url_reuses_the_current_tab),
    ]
}


// ----------------------------------------------------------------------------- real applications

fn app_window(name: &str) -> Option<isize> {
    desktop_target::find_window_by_name(name).map(|window| window as isize)
}

/// Brings the named app to the front, starting it with `open_app` when it is not running.
fn ensure_app(name: &str, title_part: &str) -> Result<(), String> {
    if let Some(window) = desktop_target::find_window_by_name(name) {
        desktop_target::bring_to_front(window);
    } else {
        act(A::OpenApp { name: name.to_owned() })?;
    }
    if wait_until(12_000, || desktop_target::foreground_title().to_lowercase().contains(&title_part.to_lowercase())) {
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    } else {
        Err(format!("'{name}' never came to the front (front window: '{}')", desktop_target::foreground_title()))
    }
}

/// A few readable labels from the screen, for failure messages.
fn screen_hint(pattern: &[&str]) -> String {
    match crate::agent::visible_text_snapshot() {
        Ok(labels) => labels
            .iter()
            .filter(|l| pattern.iter().any(|p| l.to_lowercase().contains(p)))
            .take(45)
            .map(|l| l.chars().take(60).collect::<String>())
            .collect::<Vec<_>>()
            .join(" | "),
        Err(error) => error,
    }
}

/// Notepad restores the tabs of earlier sessions; every case starts from a new empty tab.
fn fresh_notepad_tab() -> Result<(), String> {
    ensure_app("notepad", "notepad")?;
    hotkey(&["ctrl", "n"])?;
    std::thread::sleep(Duration::from_millis(600));
    Ok(())
}

fn expect_on_screen(text: &str) -> Result<(), String> {
    let wanted = text.to_lowercase();
    let mut last = String::new();
    if wait_until(8000, || match crate::agent::visible_text_snapshot() {
        Ok(labels) => {
            last = format!("{} labels, e.g. {}", labels.len(), labels.iter().filter(|l| l.chars().any(char::is_alphabetic)).take(14).map(|l| l.chars().take(40).collect::<String>()).collect::<Vec<_>>().join(" | "));
            labels.iter().any(|label| label.to_lowercase().contains(&wanted))
        }
        Err(error) => {
            last = error;
            false
        }
    }) {
        Ok(())
    } else {
        Err(format!("'{text}' is not visible on screen ({last})"))
    }
}

fn native_open_notepad(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()
}

fn native_type_lines(_lab: &Lab) -> Result<(), String> {
    ensure_app("notepad", "notepad")?;
    hotkey(&["ctrl", "a"])?;
    press("delete")?;
    type_text("first line here\nsecond line there")?;
    expect_on_screen("first line here")?;
    expect_on_screen("second line there")
}

fn native_replace_all(_lab: &Lab) -> Result<(), String> {
    ensure_app("notepad", "notepad")?;
    hotkey(&["ctrl", "a"])?;
    type_text("replacement text")?;
    expect_on_screen("replacement text")?;
    // The old text must be gone: nothing left from the earlier case.
    let labels = crate::agent::visible_text_snapshot()?;
    if labels.iter().any(|l| l.to_lowercase().contains("first line here")) {
        Err("old text is still on screen after select-all and type".into())
    } else {
        Ok(())
    }
}

fn native_long_paste(_lab: &Lab) -> Result<(), String> {
    ensure_app("notepad", "notepad")?;
    hotkey(&["ctrl", "a"])?;
    let text = "pasted paragraph number one with plenty of words in it\nand a second line of the pasted paragraph";
    type_text(text)?;
    expect_on_screen("pasted paragraph number one")?;
    expect_on_screen("second line of the pasted")
}

fn native_context_menu(_lab: &Lab) -> Result<(), String> {
    ensure_app("notepad", "notepad")?;
    let bounds = InputActuator::get_screen_bounds();
    let window = desktop_target::find_window_by_name("notepad").ok_or("no notepad window")?;
    let rect = desktop_target::visible_bounds(window).ok_or("no notepad bounds")?;
    let (x, y) = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
    let _ = bounds;
    act(A::Click { x, y, button: MouseButton::Right, clicks: 1, hold_keys: vec![] })?;
    expect_on_screen("select all")?;
    press("escape")
}

fn native_save_as_dialog(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()?;
    let folder = crate::eval::sandbox_dir();
    let _ = std::fs::create_dir_all(&folder);
    let file = folder.join("conformance_save.txt");
    let _ = std::fs::remove_file(&file);
    hotkey(&["ctrl", "a"])?;
    type_text("saved by the harness")?;
    // Save As always opens the dialog (plain Save would not for a file Notepad restored).
    hotkey(&["ctrl", "shift", "s"])?;
    std::thread::sleep(Duration::from_millis(1500));
    type_text(&file.display().to_string())?;
    press("enter")?;
    if wait_until(8000, || std::fs::read_to_string(&file).is_ok_and(|text| text.contains("saved by the harness"))) {
        // Close the now-named tab so the next session does not restore it.
        close_tab_if_front_is(&["notepad", "conformance_save"]);
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    } else {
        Err(format!("{} was not written (front window: '{}')", file.display(), desktop_target::foreground_title()))
    }
}

fn native_calculator_by_keyboard(_lab: &Lab) -> Result<(), String> {
    ensure_app("calculator", "calculator")?;
    press("escape")?;
    type_text("12+30=")?;
    expect_on_screen("display is 42").map_err(|e| format!("{e}; calculator labels: {}", screen_hint(&["display", "expression", "result", "history"])))
}

fn native_cases() -> Vec<(&'static str, &'static str, CaseFn)> {
    vec![
        ("native", "open_app notepad", native_open_notepad),
        ("native", "type two lines in notepad", native_type_lines),
        ("native", "select-all then type replaces", native_replace_all),
        ("native", "long paste with a line break", native_long_paste),
        ("native", "right-click opens a context menu", native_context_menu),
        ("native", "save through the Save As dialog", native_save_as_dialog),
        ("native", "calculator by keyboard", native_calculator_by_keyboard),
    ]
}

/// Closes windows opened during the native cases, clearing text first so no test text lingers.
fn close_new_windows(before: &[isize]) {
    // Only windows the tests themselves open are closed: Notepad, Calculator and the test pages. A
    // browser window or anything else that appeared is never touched.
    let ours = |w: &isize| {
        let hwnd = *w as windows_sys::Win32::Foundation::HWND;
        let title = desktop_target::title_of(hwnd).to_lowercase();
        let program = desktop_target::process_name_of(hwnd);
        title.contains("aura fixture") || ["notepad.exe", "calculatorapp.exe", "calc.exe"].contains(&program.as_str())
            || (title.contains("calculator") && program == "applicationframehost.exe")
    };
    let new_windows: Vec<isize> = desktop_target::list_candidate_windows().into_iter().filter(|w| !before.contains(w) && ours(w)).collect();
    for window in new_windows {
        desktop_target::bring_to_front(window as _);
        std::thread::sleep(Duration::from_millis(200));
        // Clearing text is only ever done inside Notepad: Ctrl+A then Delete in any other window
        // (File Explorer above all) would delete files.
        if desktop_target::foreground_title().to_lowercase().ends_with("notepad") {
            let _ = hotkey(&["ctrl", "a"]);
            let _ = press("delete");
        }
        desktop_target::close_window(window);
    }
    std::thread::sleep(Duration::from_millis(600));
}


// ----------------------------------------------------------------------------- orders by element label

use crate::agent::driver::{self, Order};

/// Observes the screen like the model would and carries out one order, retrying the observation
/// a few times when the element has not rendered yet.
fn go(order: Order) -> Result<String, String> {
    let mut last = String::new();
    for _ in 0..4 {
        let observation = driver::observe("")?;
        match driver::act(&observation, order) {
            Ok(report) => {
                // Keep a trail of what each order was aimed at, shown when a case fails.
                if let Ok(mut trail) = TRAIL.lock() {
                    let chosen = match order {
                        Order::Click(l) | Order::DoubleClick(l) | Order::RightClick(l) | Order::Hover(l) => {
                            observation.find(l).map(|e| format!("[{}] {} '{}'", e.id, e.kind, e.label)).unwrap_or_default()
                        }
                        _ => String::new(),
                    };
                    trail.push(format!("{order:?} {chosen} at {:?} -> front '{}': {}", report.aimed_at, report.title_after, report.effect));
                }
                return Ok(report.effect);
            }
            Err(error) if error.contains("no element labelled") => {
                last = error;
                std::thread::sleep(Duration::from_millis(700));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

static TRAIL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn trail_text() -> String {
    TRAIL.lock().map(|t| t.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join(" || ")).unwrap_or_default()
}

fn open_page(path: &str, title: &str) -> Result<(), String> {
    fx::clear();
    act(A::OpenUrl { url: format!("http://127.0.0.1:{}{path}", fx::PORT) })?;
    if wait_until(20_000, || fx::saw(&format!("load:Aura Fixture {title}"))) {
        std::thread::sleep(Duration::from_millis(900));
        // Edge may show a "Restore pages" bubble that takes the keyboard focus: Escape closes it.
        let _ = press("escape");
        std::thread::sleep(Duration::from_millis(300));
        Ok(())
    } else {
        Err(format!("the {title} page never loaded (front window: '{}')", desktop_target::foreground_title()))
    }
}

/// Closes the front tab only when the front window is one the tests opened (its title contains one of
/// `expected`). Ctrl+W in anything else would close somebody's real tab or window.
fn close_tab_if_front_is(expected: &[&str]) -> bool {
    let title = desktop_target::foreground_title().to_lowercase();
    if expected.iter().any(|part| title.contains(&part.to_lowercase())) {
        let _ = hotkey(&["ctrl", "w"]);
        true
    } else {
        false
    }
}

fn close_fixture_tab() {
    if desktop_target::foreground_title().contains("Aura Fixture") {
        let _ = hotkey(&["ctrl", "w"]);
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn driver_click_button(_lab: &Lab) -> Result<(), String> {
    open_page("/click", "Click")?;
    go(Order::Click("Beta"))?;
    expect_event("click:beta")?;
    expect_no_event("click:alpha")?;
    expect_no_event("click:gamma")
}

fn driver_fill_a_form(_lab: &Lab) -> Result<(), String> {
    open_page("/form", "Form")?;
    go(Order::Click("Name"))?;
    go(Order::Type { text: "Jane Doe", enter: false, overwrite: false })?;
    // Tab, like a person filling a form: an autofill dropdown under the name field would
    // otherwise swallow a click on the colour list.
    go(Order::Press("tab"))?;
    go(Order::Press("b"))?;
    go(Order::Click("I agree"))?;
    go(Order::Click("Submit form"))?;
    expect_event("form:name=Jane Doe;color=blue;agree=true")
}

fn driver_scroll_then_click(_lab: &Lab) -> Result<(), String> {
    open_page("/scroll", "Scroll")?;
    // No position given: the harness must aim the wheel at the page, not wherever the pointer is.
    go(Order::Scroll { clicks: -60, over: None })?;
    std::thread::sleep(Duration::from_millis(900));
    go(Order::Click("Finish"))?;
    expect_event("click:finish")
}

fn driver_double_and_right_click(_lab: &Lab) -> Result<(), String> {
    open_page("/mouse", "Mouse")?;
    go(Order::DoubleClick("Double-click me"))?;
    expect_event("dblclick:left")?;
    go(Order::RightClick("Right-click me"))?;
    expect_event("rightclick:right")
}

fn driver_follow_a_link(_lab: &Lab) -> Result<(), String> {
    open_page("/links", "Links")?;
    go(Order::Click("Pricing"))?;
    expect_event("visit:pricing")
}

fn driver_dismiss_cookie_banner(_lab: &Lab) -> Result<(), String> {
    open_page("/cookies", "Cookies")?;
    go(Order::Click("Accept"))?;
    go(Order::Click("Continue"))?;
    expect_event("click:continue")
}

fn driver_drag_between_elements(_lab: &Lab) -> Result<(), String> {
    open_page("/drag", "Drag")?;
    go(Order::Drag { from: "Drag me", to: "Drop here" })?;
    expect_event("drop:ok")
}

fn driver_hover(_lab: &Lab) -> Result<(), String> {
    open_page("/lab", "Lab")?;
    go(Order::Hover("hover me"))?;
    expect_event("hover:enter")
}

fn driver_notepad(_lab: &Lab) -> Result<(), String> {
    if desktop_target::find_window_by_name("notepad").is_none() {
        go(Order::OpenApp("notepad"))?;
        if !wait_until(12_000, || desktop_target::foreground_title().to_lowercase().contains("notepad")) {
            return Err(format!("notepad never came to the front ('{}')", desktop_target::foreground_title()));
        }
    }
    fresh_notepad_tab()?;
    let effect = go(Order::Type { text: "typed through the driver", enter: false, overwrite: false })?;
    expect_on_screen("typed through the driver").map_err(|e| format!("{e}; the harness reported: {effect}; front window: '{}'", desktop_target::foreground_title()))
}

fn driver_calculator_buttons(_lab: &Lab) -> Result<(), String> {
    if desktop_target::find_window_by_name("calculator").is_none() {
        go(Order::OpenApp("calculator"))?;
    }
    ensure_app("calculator", "calculator")?;
    go(Order::Press("escape"))?;
    for button in ["One", "Two", "Plus", "Three", "Zero", "Equals"] {
        go(Order::Click(button)).map_err(|e| format!("{button}: {e}"))?;
    }
    expect_on_screen("display is 42")
}

fn driver_elevation_check_runs(_lab: &Lab) -> Result<(), String> {
    // Every open window can be asked; the answer must never crash and our own state must be known.
    let mut elevated = 0;
    for window in desktop_target::list_candidate_windows() {
        if desktop_target::input_blocked_by_elevation(window as _).is_some() {
            elevated += 1;
        }
    }
    if desktop_target::process_is_elevated(std::process::id()).is_none() {
        return Err("the harness cannot tell whether it is elevated itself".into());
    }
    let _ = elevated;
    Ok(())
}

fn driver_browser_alert(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    go(Order::Click("Show alert"))?;
    std::thread::sleep(Duration::from_millis(700));
    go(Order::Click("OK"))?;
    expect_event("alert:closed")
}

fn driver_browser_confirm(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    go(Order::Click("Ask confirm"))?;
    std::thread::sleep(Duration::from_millis(700));
    go(Order::Click("OK"))?;
    expect_event("confirm:true")?;
    go(Order::Click("Ask confirm"))?;
    std::thread::sleep(Duration::from_millis(700));
    go(Order::Click("Cancel"))?;
    expect_event("confirm:false")
}

fn driver_link_opens_a_new_tab(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    let effect = go(Order::Click("Open in new tab"))?;
    expect_event("visit:about")?;
    let title = desktop_target::foreground_title();
    // Back to the first tab for the next case.
    close_tab_if_front_is(&["aura fixture"]);
    std::thread::sleep(Duration::from_millis(600));
    if title.contains("About") {
        Ok(())
    } else {
        Err(format!("the new tab was not in front afterwards ('{title}'); the harness reported: {effect}"))
    }
}

fn driver_hover_menu(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    go(Order::Hover("Products"))?;
    std::thread::sleep(Duration::from_millis(400));
    go(Order::Click("Pricing menu item"))?;
    expect_event("menu:pricing")
}

fn driver_content_that_arrives_late(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    go(Order::Click("Load report"))?;
    // A person (or a model) looks again after a moment; the button is not there for the first 2.5 s.
    std::thread::sleep(Duration::from_millis(3200));
    go(Order::Click("Report ready"))?;
    expect_event("click:report")
}

fn driver_disabled_button(_lab: &Lab) -> Result<(), String> {
    open_page("/tough", "Tough")?;
    let effect = go(Order::Click("Disabled action"))?;
    expect_no_event("click:disabled")?;
    // The harness must stay usable afterwards.
    go(Order::Click("Enabled action"))?;
    expect_event("click:enabled").map_err(|e| format!("{e} (after the disabled click the harness said: {effect})"))
}

fn driver_open_dialog_navigation(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()?;
    let folder = crate::eval::sandbox_dir().join("nav_target");
    let _ = std::fs::create_dir_all(&folder);
    let file = folder.join("aura-nav-target.txt");
    std::fs::write(&file, "navigated ok").map_err(|e| e.to_string())?;
    hotkey(&["ctrl", "o"])?;
    std::thread::sleep(Duration::from_millis(1500));
    // Typing a folder into the file-name box and pressing Enter walks the dialog into that folder.
    go(Order::Type { text: &folder.display().to_string(), enter: true, overwrite: false })?;
    std::thread::sleep(Duration::from_millis(1000));
    let outcome = go(Order::DoubleClick("aura-nav-target"));
    let opened = outcome.is_ok() && wait_until(8000, || desktop_target::foreground_title().to_lowercase().contains("aura-nav-target"));
    // The harness's own read-back of the focused document says what the file holds.
    let shown = if opened {
        go(Order::Click("Text editor")).and_then(|effect| {
            if effect.contains("containing 'navigated ok'") {
                Ok(())
            } else {
                Err(format!("the opened file does not hold the expected text: {effect}"))
            }
        })
    } else {
        Ok(())
    };
    let front = desktop_target::foreground_title();
    close_tab_if_front_is(&["notepad", "aura-nav-target"]);
    std::thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&folder);
    outcome.map_err(|e| format!("could not pick the file in the dialog: {e}"))?;
    if !opened {
        return Err(format!("the file was not opened (front window: '{front}')"));
    }
    shown
}

fn driver_page_shifts_before_the_click(_lab: &Lab) -> Result<(), String> {
    open_page("/shift", "Shift")?;
    let observation = driver::observe("")?;
    // The page moves everything down after the model looked (late images, ads, banners).
    press("f9")?;
    expect_event("shifted").map_err(|e| format!("the test page did not get F9: {e} (front: '{}')", desktop_target::foreground_title()))?;
    std::thread::sleep(Duration::from_millis(400));
    match driver::act(&observation, Order::Click("Target")) {
        Err(message) if message.contains("NOT EXECUTED") && message.contains("changed since you looked") => {}
        other => return Err(format!("a click aimed at a button that has since moved was not refused: {other:?}")),
    }
    expect_no_event("click:target")?;
    // Looking again gives the new position, and the click lands.
    go(Order::Click("Target"))?;
    expect_event("click:target")
}

fn driver_cases() -> Vec<(&'static str, &'static str, CaseFn)> {
    vec![
        ("driver", "click a button by label", driver_click_button),
        ("driver", "fill a form by labels", driver_fill_a_form),
        ("driver", "scroll with no position, then click", driver_scroll_then_click),
        ("driver", "double-click and right-click by label", driver_double_and_right_click),
        ("driver", "follow a link by label", driver_follow_a_link),
        ("driver", "dismiss a cookie banner", driver_dismiss_cookie_banner),
        ("driver", "drag between two labelled elements", driver_drag_between_elements),
        ("driver", "hover by label", driver_hover),
        ("driver", "type into notepad", driver_notepad),
        ("driver", "calculator by clicking buttons", driver_calculator_buttons),
        ("driver", "elevated-window check", driver_elevation_check_runs),
        ("driver", "page shifts before the click is refused, then lands", driver_page_shifts_before_the_click),
        ("driver", "browser alert is dismissed", driver_browser_alert),
        ("driver", "browser confirm answered both ways", driver_browser_confirm),
        ("driver", "link opens a new tab", driver_link_opens_a_new_tab),
        ("driver", "hover menu then click its item", driver_hover_menu),
        ("driver", "content that arrives late", driver_content_that_arrives_late),
        ("driver", "disabled button does nothing, harness stays usable", driver_disabled_button),
        ("driver", "open dialog: walk into a folder and pick a file", driver_open_dialog_navigation),
    ]
}


// ----------------------------------------------------------------------------- brain to body

fn move_window_by(window: windows_sys::Win32::Foundation::HWND, dx: i32, dy: i32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, MoveWindow};
    unsafe {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(window, &mut rect) != 0 {
            MoveWindow(window, rect.left + dx, rect.top + dy, rect.right - rect.left, rect.bottom - rect.top, 1);
        }
    }
}

fn body_refuses_when_the_window_moved(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()?;
    let observation = driver::observe("")?;
    let window = desktop_target::find_window_by_name("notepad").ok_or("no notepad window")?;
    move_window_by(window, 70, 50);
    std::thread::sleep(Duration::from_millis(400));
    let outcome = driver::act(&observation, Order::Click("Text editor"));
    move_window_by(window, -70, -50);
    match outcome {
        Err(message) if message.contains("NOT EXECUTED") && message.contains("moved or was resized") => Ok(()),
        other => Err(format!("a click aimed with an out-of-date screenshot was not refused: {other:?}")),
    }
}

fn body_restores_a_window_that_lost_focus(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()?;
    let observation = driver::observe("")?;
    // Something else takes the focus between the screenshot and the action.
    let other = desktop_target::list_candidate_windows()
        .into_iter()
        .find(|w| desktop_target::find_window_by_name("notepad").map(|n| n as isize) != Some(*w))
        .ok_or("no other window to take the focus")?;
    desktop_target::bring_to_front(other as _);
    std::thread::sleep(Duration::from_millis(500));
    let report = driver::act(&observation, Order::Type { text: "restored", enter: false, overwrite: false })?;
    if report.effect.contains("containing 'restored'") {
        Ok(())
    } else {
        Err(format!("the text did not reach Notepad: {}", report.effect))
    }
}

fn body_refuses_when_the_window_is_gone(_lab: &Lab) -> Result<(), String> {
    ensure_app("calculator", "calculator")?;
    let observation = driver::observe("")?;
    let calculator = desktop_target::find_window_by_name("calculator").ok_or("no calculator window")?;
    desktop_target::close_window(calculator as isize);
    std::thread::sleep(Duration::from_millis(900));
    match driver::act(&observation, Order::Press("1")) {
        Err(message) if message.contains("NOT EXECUTED") && message.contains("no longer in front") => Ok(()),
        other => Err(format!("a key press for a closed window was not refused: {other:?}")),
    }
}

fn body_reads_back_typed_text(lab: &Lab) -> Result<(), String> {
    let _ = lab;
    open_page("/lab", "Lab")?;
    go(Order::Click("field one"))?;
    let typed = go(Order::Type { text: "hello", enter: false, overwrite: false })?;
    if typed.contains("containing 'hello'") {
        Ok(())
    } else {
        Err(format!("the typed text was not read back: {typed}"))
    }
}

fn body_reports_a_dialog_opening_and_closing(_lab: &Lab) -> Result<(), String> {
    fresh_notepad_tab()?;
    let observation = driver::observe("")?;
    let opened = driver::act(&observation, Order::Hotkey(&["ctrl", "shift", "s"]))?;
    std::thread::sleep(Duration::from_millis(1200));
    let observation = driver::observe("")?;
    let closed = driver::act(&observation, Order::Press("escape"))?;
    if opened.effect.contains("a new window opened") && opened.effect.contains("windows open:") && closed.effect.contains("a window closed") {
        Ok(())
    } else {
        Err(format!("opening: [{}] closing: [{}]", opened.effect, closed.effect))
    }
}

fn body_refuses_a_click_covered_by_another_program(_lab: &Lab) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE};
    ensure_app("calculator", "calculator")?;
    fresh_notepad_tab()?;
    let observation = driver::observe("")?;
    let notepad = desktop_target::find_window_by_name("notepad").ok_or("no notepad window")?;
    let calculator = desktop_target::find_window_by_name("calculator").ok_or("no calculator window")?;
    let (left, top, right, bottom) = desktop_target::visible_bounds(notepad).ok_or("no notepad bounds")?;
    // Park the calculator, always on top but not active, over the middle of Notepad.
    unsafe {
        SetWindowPos(calculator, HWND_TOPMOST, (left + right) / 2 - 150, (top + bottom) / 2 - 150, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
    }
    std::thread::sleep(Duration::from_millis(600));
    let outcome = driver::act(&observation, Order::Click("Text editor"));
    unsafe {
        SetWindowPos(calculator, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | 0x0002);
    }
    desktop_target::bring_to_front(notepad);
    match outcome {
        Err(message) if message.contains("NOT EXECUTED") && message.contains("covered by") => Ok(()),
        other => Err(format!("a click on a point covered by another program was not refused: {other:?}")),
    }
}

fn body_warns_when_typing_into_a_button(lab: &Lab) -> Result<(), String> {
    let _ = lab;
    open_page("/lab", "Lab")?;
    go(Order::Click("Go"))?;
    let typed = go(Order::Type { text: "abc", enter: false, overwrite: false })?;
    if typed.contains("WARNING") {
        Ok(())
    } else {
        Err(format!("no warning that the focus is not a text field: {typed}"))
    }
}

fn minimize(window: windows_sys::Win32::Foundation::HWND) {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(window, 6) };
}

fn body_window_list_names_every_window(_lab: &Lab) -> Result<(), String> {
    ensure_app("calculator", "calculator")?;
    let active = desktop_target::effective_foreground().ok_or("no active window")?;
    let text = desktop_target::window_list_text(active);
    let active_lines = text.lines().filter(|l| l.contains("[ACTIVE")).count();
    if !text.to_lowercase().contains("calculator") || active_lines != 1 {
        return Err(format!("the window list is wrong ({active_lines} active): {text}"));
    }
    Ok(())
}

fn body_focus_a_minimized_window_by_number(_lab: &Lab) -> Result<(), String> {
    ensure_app("calculator", "calculator")?;
    let calculator = desktop_target::find_window_by_name("calculator").ok_or("no calculator window")?;
    minimize(calculator);
    std::thread::sleep(Duration::from_millis(700));
    // The orders below observe first, which numbers the windows exactly as the model's prompt does.
    let observation = driver::observe("")?;
    let active = desktop_target::effective_foreground().unwrap_or(std::ptr::null_mut());
    let listing = desktop_target::window_list_text(active);
    let number = listing
        .lines()
        .find(|l| l.to_lowercase().contains("calculator"))
        .and_then(|l| l.split('.').next())
        .and_then(|n| n.trim().parse::<u32>().ok())
        .ok_or_else(|| format!("calculator is not in the list: {listing}"))?;
    if !listing.lines().any(|l| l.to_lowercase().contains("calculator") && l.contains("minimized")) {
        return Err(format!("the minimized window is not marked as minimized: {listing}"));
    }
    driver::act(&observation, Order::FocusWindow(&number.to_string()))?;
    if wait_until(5000, || desktop_target::foreground_title().to_lowercase().contains("calculator")) {
        Ok(())
    } else {
        Err(format!("window {number} was not brought to the front (front: '{}')", desktop_target::foreground_title()))
    }
}

fn body_minimizing_is_not_closing(_lab: &Lab) -> Result<(), String> {
    ensure_app("calculator", "calculator")?;
    let before = desktop_target::window_titles();
    let calculator = desktop_target::find_window_by_name("calculator").ok_or("no calculator window")?;
    minimize(calculator);
    std::thread::sleep(Duration::from_millis(700));
    let change = desktop_target::describe_window_changes(&before);
    let _ = desktop_target::bring_to_front(calculator);
    match change {
        None => Ok(()),
        Some(text) => Err(format!("minimizing was reported as: {text}")),
    }
}

fn body_reports_scroll_position(_lab: &Lab) -> Result<(), String> {
    open_page("/scroll", "Scroll")?;
    let effect = go(Order::Scroll { clicks: -10, over: None })?;
    if effect.contains("scroll position") {
        Ok(())
    } else {
        Err(format!("a scroll was not followed by a position read-back: {effect}"))
    }
}

fn body_cases() -> Vec<(&'static str, &'static str, CaseFn)> {
    vec![
        ("body", "refuses a stale screenshot (window moved)", body_refuses_when_the_window_moved),
        ("body", "restores a window that lost focus", body_restores_a_window_that_lost_focus),
        ("body", "refuses when the window is gone", body_refuses_when_the_window_is_gone),
        ("body", "reads back typed text", body_reads_back_typed_text),
        ("body", "reports a dialog opening and closing", body_reports_a_dialog_opening_and_closing),
        ("body", "window list names every window", body_window_list_names_every_window),
        ("body", "focus a minimized window by number", body_focus_a_minimized_window_by_number),
        ("body", "minimizing is not closing", body_minimizing_is_not_closing),
        ("body", "scroll is followed by a position read-back", body_reports_scroll_position),
        ("body", "refuses a click covered by another program", body_refuses_a_click_covered_by_another_program),
        ("body", "warns when typing into a button", body_warns_when_typing_into_a_button),
    ]
}

/// Browser program the lab page is opened in (`--browser chrome`); None = the default browser.
static BROWSER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_browser(program: Option<String>) {
    if let Ok(mut browser) = BROWSER.lock() {
        *browser = program;
    }
}

/// Opens `url` in a separate browser instance with its own throwaway profile. A plain
/// `--new-window` joins the user's running browser, so closing the test window could end their
/// whole browser (2026-10-30: the user's Chrome exited the moment the test closed its window).
fn open_in_new_browser_window(program: &str, url: &str) -> Result<(), String> {
    let exe = match program.to_lowercase().as_str() {
        "chrome" => "chrome.exe",
        "edge" | "msedge" => "msedge.exe",
        other => return Err(format!("unknown browser '{other}' (chrome or edge)")),
    };
    let profile = std::env::temp_dir().join(format!("aura-test-browser-{}", exe.trim_end_matches(".exe")));
    let wide = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let arguments = format!(
        "--user-data-dir=\"{}\" --no-first-run --no-default-browser-check --disable-session-crashed-bubble --new-window {url}",
        profile.display()
    );
    let (verb, file, params) = (wide("open"), wide(exe), wide(&arguments));
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    if (result as isize) > 32 {
        Ok(())
    } else {
        Err(format!("could not start {exe} (code {})", result as isize))
    }
}

/// Opens the lab page, finds where it is on screen, and returns the geometry.
fn setup() -> Result<Lab, String> {
    if !fx::start() {
        return Err("the test page server could not start (port busy)".into());
    }
    fx::clear();
    fx::clear_rects();
    match BROWSER.lock().ok().and_then(|b| b.clone()) {
        Some(program) => open_in_new_browser_window(&program, LAB_URL)?,
        None => act(A::OpenUrl { url: LAB_URL.into() })?,
    }
    if !wait_until(25_000, || fx::rect("btn-a").is_some() && fx::rect("win").is_some()) {
        return Err("the lab page never loaded in the browser".into());
    }
    std::thread::sleep(Duration::from_millis(600));
    if let Some(window) = desktop_target::find_window_by_name("Aura Fixture") {
        desktop_target::bring_to_front(window);
    }
    std::thread::sleep(Duration::from_millis(400));

    // Calibrate: park the pointer at a known physical position inside the page and read back
    // where the page says it is. That gives the exact offset and scale, with no guesswork about
    // window borders, toolbars or display scaling.
    let bounds = InputActuator::get_screen_bounds();
    let probe_a = (bounds.x + bounds.width / 3, bounds.y + bounds.height / 3);
    let probe_b = (bounds.x + bounds.width * 2 / 3, bounds.y + bounds.height * 2 / 3);
    let read = |probe: (i32, i32)| -> Result<(f64, f64), String> {
        let before = fx::state("move");
        act(A::MouseMove { x: probe.0 + 3, y: probe.1 + 3 })?;
        act(A::MouseMove { x: probe.0, y: probe.1 })?;
        if !wait_until(2500, || fx::state("move") != before) {
            return Err("the page never reported the pointer (is the lab page in front?)".into());
        }
        std::thread::sleep(Duration::from_millis(300));
        let report = fx::state("move").unwrap_or_default();
        let mut parts = report.split(',').filter_map(|v| v.parse::<f64>().ok());
        match (parts.next(), parts.next()) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(format!("unreadable pointer report '{report}'")),
        }
    };
    let (ax, ay) = read(probe_a)?;
    let (bx, _) = read(probe_b)?;
    let span_x = bx - ax;
    if span_x.abs() < 50.0 {
        return Err(format!("the pointer did not move across the page (client x {ax} -> {bx}); is the browser window on top?"));
    }
    let dpr = (probe_b.0 - probe_a.0) as f64 / span_x;
    let off_x = probe_a.0 as f64 - ax * dpr;
    let off_y = probe_a.1 as f64 - ay * dpr;
    Ok(Lab { off_x, off_y, dpr })
}

/// Runs the cases whose group or name contains `filter` (all when None). `group:a,b` selects whole
/// groups by exact name only, so no case from another group (Notepad, Calculator, a second browser
/// page) comes along because its name happens to contain the word.
pub fn run(filter: Option<&str>, mut progress: impl FnMut(&CaseResult)) -> Result<Vec<CaseResult>, String> {
    let selected = |group: &str, name: &str| match filter {
        Some(wanted) if wanted.starts_with("group:") => wanted["group:".len()..].split(',').any(|g| g.trim().eq_ignore_ascii_case(group)),
        Some(wanted) => {
            let wanted = wanted.to_lowercase();
            group.contains(&wanted) || name.to_lowercase().contains(&wanted)
        }
        None => true,
    };
    let lab_cases: Vec<_> = cases().into_iter().filter(|(g, n, _)| selected(g, n)).collect();
    let driver_selected: Vec<_> = driver_cases().into_iter().filter(|(g, n, _)| selected(g, n)).collect();
    let app_cases: Vec<_> = driver_selected.into_iter().chain(body_cases().into_iter().filter(|(g, n, _)| selected(g, n))).chain(native_cases().into_iter().filter(|(g, n, _)| selected(g, n))).collect();
    if lab_cases.is_empty() && app_cases.is_empty() {
        return Err("no case matches that filter".into());
    }
    let windows_before = desktop_target::list_candidate_windows();
    let lab = if lab_cases.is_empty() {
        if !fx::start() {
            return Err("the test page server could not start (port busy)".into());
        }
        Lab { off_x: 0.0, off_y: 0.0, dpr: 1.0 }
    } else {
        setup()?
    };
    let mut results = Vec::new();
    let mut lab_open = !lab_cases.is_empty();
    for (group, name, case) in lab_cases.into_iter().chain(app_cases) {
        if group != "native" && group != "driver" && group != "body" {
            // lab case: nothing to close
        } else if lab_open {
            // Close the lab tab before the real-application cases.
            if desktop_target::foreground_title().contains("Aura Fixture") {
                let _ = hotkey(&["ctrl", "w"]);
                std::thread::sleep(Duration::from_millis(500));
            }
            lab_open = false;
        }
        if let Ok(mut trail) = TRAIL.lock() {
            trail.clear();
        }
        let started = Instant::now();
        let outcome = if group == "native" || group == "driver" || group == "body" { case(&lab) } else { lab.reset().and_then(|_| case(&lab)) };
        if group == "driver" || group == "body" {
            let _ = press("escape");
            close_fixture_tab();
        }
        InputActuator::release_all();
        let result = CaseResult {
            group: group.to_owned(),
            name: name.to_owned(),
            passed: outcome.is_ok(),
            millis: started.elapsed().as_millis(),
            detail: outcome
                .err()
                .map(|e| if group == "driver" || group == "body" { format!("{e} [orders: {}]", trail_text()) } else { e })
                .unwrap_or_default(),
        };
        progress(&result);
        results.push(result);
    }
    // Close the lab tab so no test page stays open.
    if lab_open && desktop_target::foreground_title().contains("Aura Fixture") {
        let _ = hotkey(&["ctrl", "w"]);
    }
    close_new_windows(&windows_before);
    Ok(results)
}

pub fn table(results: &[CaseResult]) -> String {
    let mut text = String::new();
    for r in results {
        text.push_str(&format!(
            "{:<4} {:<7} {:<42} {:>5} ms  {}\n",
            if r.passed { "ok" } else { "FAIL" },
            r.group,
            r.name,
            r.millis,
            r.detail
        ));
    }
    let passed = results.iter().filter(|r| r.passed).count();
    text.push_str(&format!("\n{passed} / {} cases passed", results.len()));
    text
}
