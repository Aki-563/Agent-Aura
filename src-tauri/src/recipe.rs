//! Remembering what worked: turns a finished run into a short recipe in plain language.
//!
//! A recipe describes each successful step by *what* was done to *which* control ("click Save"),
//! never by screen coordinates, because coordinates break the moment a window moves or the
//! display scaling changes. It is shown to the model as a hint the next time a similar task comes
//! up, so the model skips the exploring and the dead ends.

use serde_json::Value;

use crate::db::TrajectoryStepRecord;
use crate::input::ActionPrimitive;

/// One-line description of an action without any screen coordinates.
/// `target` is what the model said it was acting on ("the Save button").
pub fn describe_step(action: &ActionPrimitive, target: &str) -> Option<String> {
    let on = |verb: &str| {
        if target.trim().is_empty() {
            verb.to_owned()
        } else {
            format!("{verb} {}", target.trim())
        }
    };
    Some(match action {
        ActionPrimitive::Click { .. } => on("click"),
        ActionPrimitive::DoubleClick { .. } => on("double-click"),
        ActionPrimitive::MouseMove { .. } => on("hover over"),
        ActionPrimitive::DragAndDrop { .. } => on("drag"),
        ActionPrimitive::Scroll { clicks, .. } => format!("scroll {}", if *clicks > 0 { "up" } else { "down" }),
        ActionPrimitive::TypeText { text, press_enter, .. } => {
            let shown: String = text.chars().take(60).collect();
            format!("type \"{shown}\"{}", if *press_enter { " and press Enter" } else { "" })
        }
        ActionPrimitive::Hotkey { keys } => format!("press {}", keys.join("+")),
        ActionPrimitive::KeyPress { key } => format!("press {key}"),
        ActionPrimitive::OpenApp { name } => format!("open the app \"{name}\""),
        ActionPrimitive::FocusWindow { name } => format!("switch to the window \"{name}\""),
        ActionPrimitive::OpenUrl { url } => format!("open {url}"),
        // Bookkeeping and finishing actions are not part of a recipe.
        ActionPrimitive::WaitSettle { .. }
        | ActionPrimitive::Note { .. }
        | ActionPrimitive::AskUser { .. }
        | ActionPrimitive::Done { .. }
        | ActionPrimitive::Fail { .. } => return None,
    })
}

/// Builds the recipe from a run's stored steps (in order), keeping steps that did not fail.
pub fn build_recipe(steps: &[TrajectoryStepRecord]) -> Vec<String> {
    let mut ordered: Vec<&TrajectoryStepRecord> = steps.iter().collect();
    ordered.sort_by_key(|step| step.step_index);
    let mut recipe: Vec<String> = Vec::new();
    for step in ordered {
        if step.outcome == "failed" {
            continue;
        }
        let Ok(action) = serde_json::from_str::<ActionPrimitive>(&step.final_action_json) else {
            continue;
        };
        let target = serde_json::from_str::<Value>(&step.proposal_json)
            .ok()
            .and_then(|proposal| proposal.pointer("/target/description").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        if let Some(line) = describe_step(&action, &target) {
            if recipe.last() != Some(&line) {
                recipe.push(line);
            }
        }
    }
    recipe
}

/// Formats a stored recipe (a JSON array of strings) as a prompt hint. Returns `None` when the
/// stored text is an older format, so the caller can fall back to showing it as is.
pub fn format_hint(stored_json: &str) -> Option<String> {
    let steps: Vec<String> = serde_json::from_str(stored_json).ok()?;
    if steps.is_empty() {
        return None;
    }
    let lines = steps
        .iter()
        .enumerate()
        .map(|(index, step)| format!("{}. {step}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "A SIMILAR TASK WAS DONE SUCCESSFULLY BEFORE. These are the steps that worked (adapt them to what is on screen now; the screen may differ, and do not repeat steps that are already done):\n{lines}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(index: i32, outcome: &str, action: Value, target: &str) -> TrajectoryStepRecord {
        TrajectoryStepRecord {
            run_id: "run".into(),
            step_index: index,
            proposal_json: json!({"target": {"description": target}}).to_string(),
            verification_json: "{}".into(),
            final_action_json: action.to_string(),
            outcome: outcome.into(),
            expected_outcome: String::new(),
            screenshot_webp: None,
            window_title: None,
        }
    }

    #[test]
    fn recipes_drop_failed_steps_and_never_contain_coordinates() {
        let steps = vec![
            step(3, "succeeded", json!({"type": "type_text", "text": "hello", "press_enter": false}), ""),
            step(1, "succeeded", json!({"type": "open_app", "name": "notepad"}), "Notepad"),
            step(2, "failed", json!({"type": "click", "x": 812, "y": 640}), "the Share button"),
            step(4, "uncertain", json!({"type": "click", "x": 100, "y": 200}), "the File menu"),
            step(5, "succeeded", json!({"type": "hotkey", "keys": ["ctrl", "s"]}), ""),
            step(6, "succeeded", json!({"type": "done", "summary": "saved"}), ""),
        ];
        let recipe = build_recipe(&steps);
        assert_eq!(
            recipe,
            ["open the app \"notepad\"", "type \"hello\"", "click the File menu", "press ctrl+s"]
        );
        assert!(recipe.iter().all(|line| !line.contains("812") && !line.contains("640") && !line.contains("100")));
    }

    #[test]
    fn consecutive_duplicates_collapse_and_unparseable_steps_are_skipped() {
        let mut steps = vec![
            step(1, "succeeded", json!({"type": "key_press", "key": "tab"}), ""),
            step(2, "succeeded", json!({"type": "key_press", "key": "tab"}), ""),
        ];
        let mut broken = step(3, "succeeded", json!({}), "");
        broken.final_action_json = "not json".into();
        steps.push(broken);
        assert_eq!(build_recipe(&steps), ["press tab"]);
        assert!(build_recipe(&[]).is_empty());
    }

    #[test]
    fn the_hint_is_numbered_and_old_formats_are_left_alone() {
        let hint = format_hint(r#"["open the app \"notepad\"", "type \"hi\""]"#).unwrap();
        assert!(hint.contains("1. open the app \"notepad\"") && hint.contains("2. type \"hi\""));
        assert!(hint.starts_with("A SIMILAR TASK WAS DONE SUCCESSFULLY BEFORE"));
        assert!(format_hint("[]").is_none());
        assert!(format_hint(r#"[{"action": {"type": "click", "x": 1, "y": 2}}]"#).is_none(), "old raw format");
    }
}
