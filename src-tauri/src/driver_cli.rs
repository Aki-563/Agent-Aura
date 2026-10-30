//! `aura-cli look` and `aura-cli do`: a human (or an assistant) acts as the model.
//!
//! `look` shows the screen exactly as the model would see it: the active window, the numbered
//! elements, and a screenshot with the numbered boxes drawn on it (saved to a PNG file).
//! `do <order>` carries out one order through the same observe, number, scale, guard, execute and
//! settle path the agent uses, prints the measured effect, then shows the screen again.

use std::time::Duration;

use crate::agent::driver::{self, Observation, Order};

pub fn screenshot_path() -> std::path::PathBuf {
    std::env::temp_dir().join("aura-look.png")
}

fn show(observation: &Observation) {
    let path = screenshot_path();
    let saved = std::fs::write(&path, observation.screenshot_png()).is_ok();
    let (w, h) = observation.size();
    println!("window: {}", observation.window_title());
    let active = crate::desktop::target::effective_foreground().unwrap_or(std::ptr::null_mut());
    println!("windows open: {}", crate::desktop::target::window_list_text(active));
    println!("scroll: {}", crate::perception::scroll_position_hint().unwrap_or_else(|| "not reported by this app".to_owned()));
    println!("screenshot: {w}x{h}{}", if saved { format!(" saved to {}", path.display()) } else { String::new() });
    let elements = observation.elements();
    println!("elements ({}):", elements.len());
    for element in elements.iter().take(80) {
        println!("[{}] {} \"{}\" ({},{})-({},{})", element.id, element.kind, element.label, element.left, element.top, element.right, element.bottom);
    }
    if elements.len() > 80 {
        println!("... {} more", elements.len() - 80);
    }
}

fn number(text: &str, what: &str) -> Result<i32, String> {
    text.trim().parse().map_err(|_| format!("{what} must be a whole number, got '{text}'"))
}

fn usage() -> String {
    "usage:\n  aura-cli look [--task <text>]\n  aura-cli do <order>\norders: click <label> | dblclick <label> | rclick <label> | mclick <label> | shiftclick <label> | ctrlclick <label>\n  hover <label> | clickxy <x> <y> | drag <from label> <to label> | dragxy <x1> <y1> <x2> <y2>\n  scroll <clicks> [over <label>] | type <text> [--enter] [--overwrite] | press <key> | hotkey <key> <key>...\n  open_app <name> | open_url <url> | focus <name> | wait <ms>\n(coordinates are screenshot coordinates; negative scroll goes down)"
        .to_owned()
}

pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

fn run_inner(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else { return Err(usage()) };
    match command.as_str() {
        "look" => {
            let task = args.iter().position(|a| a == "--task").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            let started = std::time::Instant::now();
            let observation = driver::observe(&task)?;
            println!("observed in {} ms", started.elapsed().as_millis());
            show(&observation);
            if let Some(position) = args.iter().position(|a| a == "--candidates") {
                let needle = args.get(position + 1).map(|w| w.to_lowercase()).unwrap_or_default();
                for (kind, label, (l, t, r, b)) in observation.all_candidates() {
                    if needle.is_empty() || label.to_lowercase().contains(&needle) {
                        println!("candidate {kind} \"{label}\" ({l},{t})-({r},{b})");
                    }
                }
            }
            Ok(())
        }
        "do" => {
            // `--task <text>` tells the screen reader what the task is about, like the real task text does.
            let task_text = args.iter().position(|a| a == "--task").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            let rest_args: Vec<String> = match args.iter().position(|a| a == "--task") {
                Some(i) => args.iter().enumerate().filter(|(k, _)| *k != i && *k != i + 1).map(|(_, a)| a.clone()).collect(),
                None => args.to_vec(),
            };
            let words: Vec<&str> = rest_args[1..].iter().map(String::as_str).collect();
            let flags_enter = words.contains(&"--enter");
            let flags_overwrite = words.contains(&"--overwrite");
            let words: Vec<&str> = words.into_iter().filter(|w| *w != "--enter" && *w != "--overwrite").collect();
            let Some((verb, rest)) = words.split_first() else { return Err(usage()) };
            if *verb == "wait" {
                let ms = number(rest.first().ok_or("wait needs milliseconds")?, "wait")?;
                std::thread::sleep(Duration::from_millis(ms.max(0) as u64));
                show(&driver::observe("")?);
                return Ok(());
            }
            let hold_shift = ["shift"];
            let hold_ctrl = ["ctrl"];
            let need = |index: usize, what: &str| rest.get(index).copied().ok_or_else(|| format!("{verb} needs {what}"));
            let joined = rest.join(" ");
            let keys: Vec<&str> = rest.to_vec();
            let order = match *verb {
                "click" => Order::Click(&joined),
                "dblclick" => Order::DoubleClick(&joined),
                "rclick" => Order::RightClick(&joined),
                "mclick" => Order::MiddleClick(&joined),
                "shiftclick" => Order::ClickHolding(&joined, &hold_shift),
                "ctrlclick" => Order::ClickHolding(&joined, &hold_ctrl),
                "hover" => Order::Hover(&joined),
                "clickxy" => Order::ClickAt(number(need(0, "x")?, "x")?, number(need(1, "y")?, "y")?),
                "dragxy" => Order::DragAt(
                    number(need(0, "x1")?, "x1")?,
                    number(need(1, "y1")?, "y1")?,
                    number(need(2, "x2")?, "x2")?,
                    number(need(3, "y2")?, "y2")?,
                ),
                "drag" => Order::Drag { from: need(0, "a from-label")?, to: need(1, "a to-label")? },
                "scroll" => {
                    let clicks = number(need(0, "a click count")?, "scroll clicks")?;
                    let over = rest.get(1).copied().filter(|w| *w != "over").or_else(|| rest.get(2).copied());
                    Order::Scroll { clicks, over }
                }
                "type" => Order::Type { text: &joined, enter: flags_enter, overwrite: flags_overwrite },
                "press" => Order::Press(need(0, "a key")?),
                "hotkey" => Order::Hotkey(&keys),
                "open_app" => Order::OpenApp(&joined),
                "open_url" => Order::OpenUrl(need(0, "an address")?),
                "focus" => Order::FocusWindow(&joined),
                other => return Err(format!("unknown order '{other}'\n{}", usage())),
            };
            let observation = driver::observe(&task_text)?;
            match driver::act(&observation, order) {
                Ok(report) => {
                    println!("done in {} ms. effect: {}", report.millis, report.effect);
                    if let Some((x, y)) = report.aimed_at {
                        println!("aimed at physical ({x}, {y})");
                    }
                }
                Err(error) => println!("REFUSED OR FAILED: {error}"),
            }
            std::thread::sleep(Duration::from_millis(350));
            show(&driver::observe("")?);
            Ok(())
        }
        "demo" => demo(),
        "serve" => serve(args),
        _ => Err(usage()),
    }
}

/// `aura-cli serve [--browser chrome|edge] [--page /lab] [--seconds N]`: runs the test-page server
/// (so `look` and `do` can be tried on the fixture pages) and opens the page in an isolated
/// browser instance with a throwaway profile, never in the user's own browser.
fn serve(args: &[String]) -> Result<(), String> {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let seconds: u64 = value("--seconds").and_then(|s| s.parse().ok()).unwrap_or(900);
    if !crate::eval_fixture::start() {
        return Err("the test page server could not start (port busy)".into());
    }
    let page = value("--page").unwrap_or_else(|| "/lab".to_owned());
    let url = format!("http://127.0.0.1:{}{page}", crate::eval_fixture::PORT);
    println!("serving the test pages at http://127.0.0.1:{} for {seconds} s", crate::eval_fixture::PORT);
    if let Some(browser) = value("--browser") {
        crate::conformance::open_in_new_browser_window(&browser, &url)?;
        println!("opened {url} in an isolated {browser} window");
    }
    std::thread::sleep(Duration::from_secs(seconds));
    Ok(())
}

/// Shows the HUD states with their top-centre labels, glides the pointer between a few points and
/// plays the start and finish sounds, so the look and feel can be checked without a model.
fn demo() -> Result<(), String> {
    use crate::overlay::{GlowRenderer, HudState, OverlayWindow};
    use crate::input::send_input::InputActuator;
    let window = std::sync::Arc::new(OverlayWindow::new()?);
    let mut renderer = GlowRenderer::new(window);
    renderer.start()?;
    crate::sound::play(crate::sound::Sound::Start);
    let bounds = InputActuator::get_screen_bounds();
    let point = |fx: f64, fy: f64| (bounds.x + (bounds.width as f64 * fx) as i32, bounds.y + (bounds.height as f64 * fy) as i32);
    let shot = |name: &str| {
        if let Ok(observation) = driver::observe("") {
            let path = std::env::temp_dir().join(format!("aura-demo-{name}.png"));
            let _ = std::fs::write(&path, observation.screenshot_png());
            println!("  screenshot: {}", path.display());
        }
    };
    for (state, target) in [
        (HudState::Capturing, point(0.5, 0.5)),
        (HudState::Thinking, point(0.25, 0.35)),
        (HudState::Working, point(0.75, 0.65)),
        (HudState::Settling, point(0.4, 0.7)),
    ] {
        println!("{state:?}: {}", state.label().unwrap_or(""));
        renderer.set_state(state);
        std::thread::sleep(Duration::from_millis(400));
        InputActuator::move_to_confirmed(target.0, target.1).map_err(|e| e.to_string())?;
        if state == HudState::Working {
            shot("working");
        }
        std::thread::sleep(Duration::from_millis(900));
    }
    println!("Finished: Task finished");
    renderer.set_state(HudState::Finished);
    crate::sound::play(crate::sound::Sound::Finished);
    std::thread::sleep(Duration::from_millis(700));
    shot("finished");
    std::thread::sleep(Duration::from_millis(2300));
    renderer.set_state(HudState::Inactive);
    renderer.stop();
    Ok(())
}
