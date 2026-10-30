//! Structured-output schemas and tolerant parsing for model responses.
//!
//! Local servers (LM Studio, llama.cpp) reject `response_format: json_object` but accept
//! `json_schema`, which also forces small models to emit valid JSON. Model output is still
//! never trusted: it is parsed leniently (floats for integers, extra fields, prose around the
//! JSON) and then validated into the strict typed `ActionPrimitive` with error messages
//! specific enough to feed back to the model on a retry.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::input::{ActionPrimitive, MouseButton};
use crate::reliability::{
    ActionProposal, ImpactClass, StepOutcome, TargetDescriptor, VerificationVerdict, VerifiedAction,
};

const ACTION_TYPES: [&str; 16] = [
    "click",
    "double_click",
    "mouse_move",
    "drag_and_drop",
    "type_text",
    "hotkey",
    "key_press",
    "open_app",
    "focus_window",
    "open_url",
    "note",
    "ask_user",
    "scroll",
    "wait_settle",
    "done",
    "fail",
];

/// One action variant: a closed set of fields (`additionalProperties: false`, so a constrained
/// decoder cannot invent mangled names like `keys_`) with the fields this type needs required.
fn action_variant(kind: &str, properties: Value, required: &[&str]) -> Value {
    let mut props = serde_json::Map::new();
    props.insert("type".to_owned(), json!({"type": "string", "enum": [kind]}));
    if let Value::Object(extra) = properties {
        props.extend(extra);
    }
    let mut required_all = vec!["type"];
    required_all.extend_from_slice(required);
    json!({
        "type": "object",
        "properties": props,
        "required": required_all,
        "additionalProperties": false
    })
}

/// Action schema shared by the proposal and verification schemas: one variant per action type
/// so the decoder itself enforces per-type required fields. Conversion into the typed
/// `ActionPrimitive` still validates everything (`LooseAction::into_primitive`).
fn action_schema(max_element: u32) -> Value {
    let point = json!({"x": {"type": "integer"}, "y": {"type": "integer"}});
    let with = |base: &Value, extra: Value| {
        let mut merged = base.as_object().cloned().unwrap_or_default();
        if let Value::Object(more) = extra {
            merged.extend(more);
        }
        Value::Object(merged)
    };
    let button = json!({"button": {"type": "string", "enum": ["left", "right", "middle"]}});
    // Keys held during the click or drag (for example ["shift"] to extend a selection).
    let hold = json!({"hold_keys": {"type": "array", "items": {"type": "string"}}});
    // An element number is only offered when elements were detected, and is bounded to the real
    // range so a constrained decoder cannot invent one.
    let element = json!({"type": "integer", "minimum": 1, "maximum": max_element});

    // Cheap, robust actions first (small models lean toward the first option): application and
    // keyboard actions, then pointer actions, then bookkeeping and the finishing actions.
    let mut variants = vec![
        action_variant("open_app", json!({"name": {"type": "string", "maxLength": 60}}), &["name"]),
        action_variant("focus_window", json!({"name": {"type": "string", "maxLength": 80}}), &["name"]),
        action_variant("open_url", json!({"url": {"type": "string", "maxLength": 400}}), &["url"]),
        action_variant("hotkey", json!({"keys": {"type": "array", "items": {"type": "string"}}}), &["keys"]),
        action_variant("key_press", json!({"key": {"type": "string"}}), &["key"]),
        action_variant("type_text", json!({
            "text": {"type": "string"},
            "press_enter": {"type": "boolean"},
            "overwrite": {"type": "boolean"}
        }), &["text"]),
        action_variant("click", with(&with(&with(&point, button.clone()), json!({"clicks": {"type": "integer"}})), hold.clone()), &["x", "y"]),
        action_variant("double_click", with(&point, button.clone()), &["x", "y"]),
        action_variant("mouse_move", point.clone(), &["x", "y"]),
        action_variant("drag_and_drop", json!({
            "from_x": {"type": "integer"}, "from_y": {"type": "integer"},
            "to_x": {"type": "integer"}, "to_y": {"type": "integer"},
            "duration_ms": {"type": "integer"},
            "hold_keys": {"type": "array", "items": {"type": "string"}}
        }), &["from_x", "from_y", "to_x", "to_y"]),
        action_variant("scroll", with(&point, json!({
            "clicks": {"type": "integer"},
            "horizontal": {"type": "boolean"}
        })), &["clicks"]),
        action_variant("wait_settle", json!({"max_timeout_ms": {"type": "integer"}, "seconds": {"type": "number"}}), &[]),
        action_variant("note", json!({"text": {"type": "string", "maxLength": 300}}), &["text"]),
        action_variant("ask_user", json!({"question": {"type": "string", "maxLength": 240}}), &["question"]),
        action_variant("done", json!({"summary": {"type": "string"}}), &["summary"]),
        action_variant("fail", json!({"reason": {"type": "string"}}), &["reason"]),
    ];
    if max_element > 0 {
        // Preferred: point at a numbered element instead of estimating pixels.
        let pointer_start = variants.iter().position(|v| v["properties"]["type"]["enum"][0] == "click").unwrap_or(0);
        variants.insert(pointer_start, action_variant("click", with(&json!({"element": element.clone()}), with(&with(&button, json!({"clicks": {"type": "integer"}})), hold.clone())), &["element"]));
        variants.insert(pointer_start + 1, action_variant("double_click", with(&json!({"element": element.clone()}), button), &["element"]));
        variants.insert(pointer_start + 2, action_variant("mouse_move", json!({"element": element.clone()}), &["element"]));
        variants.push(action_variant("drag_and_drop", json!({
            "from_element": element.clone(), "to_element": element.clone(),
            "duration_ms": {"type": "integer"}
        }), &["from_element", "to_element"]));
        variants.push(action_variant("scroll", json!({
            "element": element,
            "clicks": {"type": "integer"},
            "horizontal": {"type": "boolean"}
        }), &["element", "clicks"]));
    }
    json!({ "anyOf": variants })
}

pub fn proposal_schema(max_element: u32) -> Value {
    json!({
        "type": "object",
        "properties": {
            "stepId": {"type": "integer"},
            "previousOutcome": {"type": "string", "enum": ["not_applicable", "succeeded", "failed", "uncertain"]},
            "reflection": {"type": "string", "maxLength": 240},
            "thought": {"type": "string", "maxLength": 240},
            "target": {
                "type": "object",
                "properties": {
                    "description": {"type": "string", "maxLength": 120},
                    "visibleText": {"type": "string", "maxLength": 80},
                    "controlType": {"type": "string", "maxLength": 40}
                },
                "required": ["description"],
                "additionalProperties": false
            },
            "expectedOutcome": {"type": "string", "maxLength": 160},
            "action": action_schema(max_element),
            "confidence": {"type": "number"},
            "impact": {"type": "string", "enum": ["routine", "destructive", "financial", "credential", "communication"]}
        },
        "required": ["previousOutcome", "reflection", "thought", "target", "expectedOutcome", "action", "impact"],
        "additionalProperties": false
    })
}

pub fn verification_schema(max_element: u32) -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["accept", "correct", "replan", "fail"]},
            "reason": {"type": "string"},
            "finalAction": action_schema(max_element),
            "confidence": {"type": "number"},
            "groundingEvidence": {"type": "string"}
        },
        "required": ["verdict", "reason"],
        "additionalProperties": false
    })
}

pub fn done_check_schema(_max_element: u32) -> Value {
    json!({
        "type": "object",
        "properties": {
            "evidence": {"type": "string", "maxLength": 240},
            "complete": {"type": "boolean"},
            "missing": {"type": "string", "maxLength": 200}
        },
        "required": ["evidence", "complete", "missing"],
        "additionalProperties": false
    })
}

/// Result of the strict completion check that runs when the model claims the task is done.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DoneCheck {
    pub evidence: String,
    pub complete: bool,
    pub missing: String,
}

pub fn parse_done_check(raw_text: &str) -> Result<DoneCheck, String> {
    parse_lenient(raw_text, "completion check")
}

pub fn final_outcome_schema(_max_element: u32) -> Value {
    json!({
        "type": "object",
        "properties": {
            "outcome": {"type": "string", "enum": ["succeeded", "failed", "uncertain"]},
            "reflection": {"type": "string"},
            "completed": {"type": "boolean"},
            "summary": {"type": "string"}
        },
        "required": ["outcome", "reflection", "completed", "summary"],
        "additionalProperties": false
    })
}

/// Which schema (if any) accompanies a model request.
#[derive(Debug, Clone, Copy)]
pub struct SchemaSpec {
    pub name: &'static str,
    pub build: fn(u32) -> Value,
    /// Highest valid element number for this observation (0 = no numbered elements).
    pub max_element: u32,
}

impl SchemaSpec {
    pub const fn with_max_element(self, max_element: u32) -> Self {
        Self { max_element, ..self }
    }
}

pub const PROPOSAL: SchemaSpec = SchemaSpec { name: "aura_proposal", build: proposal_schema, max_element: 0 };
pub const VERIFICATION: SchemaSpec = SchemaSpec { name: "aura_verification", build: verification_schema, max_element: 0 };
pub const DONE_CHECK: SchemaSpec = SchemaSpec { name: "aura_done_check", build: done_check_schema, max_element: 0 };
pub const FINAL_OUTCOME: SchemaSpec = SchemaSpec { name: "aura_final_outcome", build: final_outcome_schema, max_element: 0 };

/// End index (inclusive) of the balanced `{...}` starting at `start`, string-aware.
fn balanced_object_end(raw: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in raw.as_bytes()[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the first complete top-level JSON *object that actually parses* inside model text.
/// Handles code fences, `<think>` blocks (which may contain stray `{braces}`) and prose before
/// or after the object. If the first top-level object is never closed (a reply cut off by the
/// token limit) it returns `None` rather than an object nested inside the broken one.
pub fn extract_json_object(raw: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(offset) = raw[from..].find('{') {
        let start = from + offset;
        let end = balanced_object_end(raw, start)?;
        let candidate = &raw[start..=end];
        if serde_json::from_str::<Value>(candidate).map(|v| v.is_object()).unwrap_or(false) {
            return Some(candidate);
        }
        from = end + 1;
    }
    None
}

/// Parses model text into `T`, tolerating surrounding prose and markdown fences.
pub fn parse_lenient<T: for<'de> Deserialize<'de>>(raw: &str, label: &str) -> Result<T, String> {
    let json_text = extract_json_object(raw).ok_or_else(|| {
        if raw.contains('{') {
            format!("Your {label} was cut off or was not complete JSON (it probably ran too long). Keep every text field to ONE short sentence and reply with a single complete JSON object.")
        } else {
            format!("Your {label} did not contain a JSON object. Reply with one JSON object only.")
        }
    })?;
    serde_json::from_str(json_text)
        .map_err(|error| format!("Your {label} JSON had the wrong shape: {error}"))
}

/// Action as models actually emit it: every field optional, numbers may be floats.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LooseAction {
    #[serde(rename = "type")]
    pub kind: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub button: Option<String>,
    pub clicks: Option<f64>,
    pub from_x: Option<f64>,
    pub from_y: Option<f64>,
    pub to_x: Option<f64>,
    pub to_y: Option<f64>,
    pub duration_ms: Option<f64>,
    pub text: Option<String>,
    pub press_enter: Option<bool>,
    pub overwrite: Option<bool>,
    pub keys: Option<Vec<String>>,
    pub key: Option<String>,
    pub url: Option<String>,
    pub question: Option<String>,
    pub seconds: Option<f64>,
    pub element: Option<f64>,
    pub from_element: Option<f64>,
    pub to_element: Option<f64>,
    pub name: Option<String>,
    pub hold_keys: Option<Vec<String>>,
    pub horizontal: Option<bool>,
    pub max_timeout_ms: Option<f64>,
    pub summary: Option<String>,
    pub reason: Option<String>,
}

fn to_i32(value: f64) -> i32 {
    value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// Maps an element id from the marked screenshot to a point (screenshot pixels).
pub type ElementResolver<'a> = &'a dyn Fn(u32) -> Result<(i32, i32), String>;

impl LooseAction {
    /// Replaces element ids by the centre of that element, so later steps only see points.
    pub fn resolve_elements(&mut self, resolve: ElementResolver) -> Result<(), String> {
        let id_of = |value: f64| value.round().max(0.0) as u32;
        if let Some(element) = self.element {
            let (x, y) = resolve(id_of(element))?;
            self.x = Some(x as f64);
            self.y = Some(y as f64);
        }
        if let Some(element) = self.from_element {
            let (x, y) = resolve(id_of(element))?;
            self.from_x = Some(x as f64);
            self.from_y = Some(y as f64);
        }
        if let Some(element) = self.to_element {
            let (x, y) = resolve(id_of(element))?;
            self.to_x = Some(x as f64);
            self.to_y = Some(y as f64);
        }
        Ok(())
    }

    /// Converts into a typed action, or explains exactly what is missing or invalid.
    pub fn into_primitive(self) -> Result<ActionPrimitive, String> {
        let kind = self.kind.trim().to_lowercase().replace([' ', '-'], "_");
        let need_point = |x: Option<f64>, y: Option<f64>, what: &str| -> Result<(i32, i32), String> {
            match (x, y) {
                (Some(x), Some(y)) => Ok((to_i32(x), to_i32(y))),
                _ => Err(format!("{what} requires numeric x and y screenshot coordinates.")),
            }
        };
        let button = match self.button.as_deref().map(|b| b.trim().to_lowercase()) {
            None => MouseButton::Left,
            Some(b) if b == "left" || b.is_empty() => MouseButton::Left,
            Some(b) if b == "right" => MouseButton::Right,
            Some(b) if b == "middle" => MouseButton::Middle,
            Some(other) => return Err(format!("Unknown mouse button '{other}'. Use left, right or middle.")),
        };
        let hold_keys = self.hold_keys.clone().unwrap_or_default();

        match kind.as_str() {
            "click" | "left_click" | "right_click" | "middle_click" => {
                let (x, y) = need_point(self.x, self.y, "click")?;
                let button = match kind.as_str() {
                    "right_click" => MouseButton::Right,
                    "middle_click" => MouseButton::Middle,
                    _ => button,
                };
                let clicks = self.clicks.map(|c| c.round().clamp(1.0, 3.0) as u32).unwrap_or(1);
                Ok(ActionPrimitive::Click { x, y, button, clicks, hold_keys })
            }
            "mouse_move" | "move" | "hover" | "move_mouse" | "mouse_hover" => {
                let (x, y) = need_point(self.x, self.y, "mouse_move")?;
                Ok(ActionPrimitive::MouseMove { x, y })
            }
            "double_click" | "doubleclick" => {
                let (x, y) = need_point(self.x, self.y, "double_click")?;
                Ok(ActionPrimitive::DoubleClick { x, y, button })
            }
            "drag_and_drop" | "drag" | "drag_drop" => {
                let (from_x, from_y) = need_point(self.from_x, self.from_y, "drag_and_drop start (from_x, from_y)")?;
                let (to_x, to_y) = need_point(self.to_x, self.to_y, "drag_and_drop end (to_x, to_y)")?;
                let duration_ms = self.duration_ms.map(|d| d.round().clamp(100.0, 5000.0) as u64).unwrap_or(250);
                Ok(ActionPrimitive::DragAndDrop { from_x, from_y, to_x, to_y, duration_ms, hold_keys })
            }
            "type_text" | "type" | "text" | "write" => {
                let text = self.text.ok_or("type_text requires a 'text' string.")?;
                Ok(ActionPrimitive::TypeText {
                    text,
                    press_enter: self.press_enter.unwrap_or(false),
                    overwrite: self.overwrite.unwrap_or(false),
                })
            }
            "hotkey" | "shortcut" => {
                let keys = self
                    .keys
                    .or_else(|| self.key.clone().map(|k| vec![k]))
                    .map(split_key_combos)
                    .filter(|k| !k.is_empty())
                    .ok_or("hotkey requires a non-empty 'keys' array, e.g. [\"ctrl\",\"l\"].")?;
                Ok(ActionPrimitive::Hotkey { keys })
            }
            "key_press" | "key" | "press" => {
                let key = self
                    .key
                    .or_else(|| self.keys.and_then(|k| k.into_iter().next()))
                    .filter(|k| !k.trim().is_empty())
                    .ok_or("key_press requires a 'key' string, e.g. \"enter\".")?;
                // "ctrl+s" written as a single key is a shortcut, not a key called "ctrl+s".
                let parts = split_key_combos(vec![key.clone()]);
                if parts.len() > 1 {
                    return Ok(ActionPrimitive::Hotkey { keys: parts });
                }
                Ok(ActionPrimitive::KeyPress { key })
            }
            "open_app" | "open" | "launch" | "launch_app" | "start_app" => {
                let name = self
                    .name
                    .or(self.text)
                    .filter(|n| !n.trim().is_empty())
                    .ok_or("open_app requires the application name in 'name', e.g. \"notepad\".")?;
                Ok(ActionPrimitive::OpenApp { name })
            }
            "focus_window" | "switch_window" | "switch_to" | "focus" | "activate_window" => {
                let name = self
                    .name
                    .filter(|n| !n.trim().is_empty())
                    .ok_or("focus_window requires the window title or program name in 'name', e.g. \"chrome\".")?;
                Ok(ActionPrimitive::FocusWindow { name })
            }
            "open_url" | "goto" | "navigate" | "browse" | "open_website" => {
                let url = self
                    .url
                    .or(self.text)
                    .filter(|u| !u.trim().is_empty())
                    .ok_or("open_url requires an http or https address in 'url'.")?;
                Ok(ActionPrimitive::OpenUrl { url })
            }
            "note" | "remember" | "save_note" | "save_to_knowledge" => {
                let text = self
                    .text
                    .or(self.summary)
                    .filter(|t| !t.trim().is_empty())
                    .ok_or("note requires the text to remember in 'text'.")?;
                Ok(ActionPrimitive::Note { text })
            }
            "ask_user" | "ask" | "ask_for_help" | "request_help" | "help" => {
                let question = self
                    .question
                    .or(self.text)
                    .or(self.reason)
                    .filter(|q| !q.trim().is_empty())
                    .ok_or("ask_user requires what you need from the user in 'question'.")?;
                Ok(ActionPrimitive::AskUser { question })
            }
            "scroll" => {
                let clicks = self.clicks.ok_or("scroll requires an integer 'clicks' (positive up, negative down).")?;
                let (x, y) = match (self.x, self.y) {
                    (Some(x), Some(y)) => (Some(to_i32(x)), Some(to_i32(y))),
                    (None, None) => (None, None),
                    _ => return Err("scroll needs both x and y, or neither.".to_owned()),
                };
                Ok(ActionPrimitive::Scroll {
                    clicks: to_i32(clicks).clamp(-50, 50),
                    x,
                    y,
                    horizontal: self.horizontal.unwrap_or(false),
                })
            }
            "wait_settle" | "wait" | "sleep" => Ok(ActionPrimitive::WaitSettle {
                max_timeout_ms: self
                    .max_timeout_ms
                    .or(self.seconds.map(|s| s * 1000.0))
                    .map(|v| v.round().clamp(100.0, 10_000.0) as u64)
                    .unwrap_or(1000),
                expected_cursor: None,
            }),
            "done" | "finish" | "complete" => Ok(ActionPrimitive::Done {
                summary: self.summary.or(self.reason).unwrap_or_else(|| "Task complete.".to_owned()),
            }),
            "fail" | "give_up" => Ok(ActionPrimitive::Fail {
                reason: self.reason.or(self.summary).unwrap_or_else(|| "The task cannot be completed.".to_owned()),
            }),
            "" => Err("The action has no 'type'.".to_owned()),
            other => Err(format!(
                "Unknown action type '{other}'. Allowed: {}.",
                ACTION_TYPES.join(", ")
            )),
        }
    }
}

/// Models write shortcuts as `["ctrl","s"]`, `["Ctrl+S"]` or `"ctrl + shift + t"`. Splits any
/// token that joins keys with `+` (a lone "+" stays the plus key) into separate key names.
fn split_key_combos(keys: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        let key = key.trim();
        if key.len() > 1 && key.contains('+') {
            let ends_with_plus_key = key.ends_with("++");
            let body = if ends_with_plus_key { &key[..key.len() - 2] } else { key };
            out.extend(body.split('+').map(str::trim).filter(|part| !part.is_empty()).map(str::to_owned));
            if ends_with_plus_key {
                out.push("+".to_owned());
            }
        } else if !key.is_empty() {
            out.push(key.to_owned());
        }
    }
    out
}

fn parse_outcome(value: Option<&str>) -> StepOutcome {
    match value.map(|v| v.trim().to_lowercase()).as_deref() {
        Some("succeeded") | Some("success") => StepOutcome::Succeeded,
        Some("failed") | Some("failure") => StepOutcome::Failed,
        Some("uncertain") => StepOutcome::Uncertain,
        _ => StepOutcome::NotApplicable,
    }
}

fn parse_impact(value: Option<&str>) -> ImpactClass {
    match value.map(|v| v.trim().to_lowercase()).as_deref() {
        Some("destructive") => ImpactClass::Destructive,
        Some("financial") => ImpactClass::Financial,
        Some("credential") => ImpactClass::Credential,
        Some("communication") => ImpactClass::Communication,
        _ => ImpactClass::Routine,
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RawTarget {
    description: String,
    visible_text: Option<String>,
    control_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RawProposal {
    step_id: Option<f64>,
    previous_outcome: Option<String>,
    reflection: String,
    thought: String,
    target: RawTarget,
    expected_outcome: String,
    action: LooseAction,
    confidence: Option<f64>,
    impact: Option<String>,
}

pub fn parse_proposal(raw_text: &str) -> Result<ActionProposal, String> {
    parse_proposal_with(raw_text, &|id| Err(format!("Element {id} is not available; use x and y instead.")))
}

pub fn parse_proposal_with(raw_text: &str, resolve: ElementResolver) -> Result<ActionProposal, String> {
    let mut raw: RawProposal = parse_lenient(raw_text, "action proposal")?;
    raw.action.resolve_elements(resolve)?;
    let action = raw.action.into_primitive()?;
    Ok(ActionProposal {
        step_id: raw.step_id.map(|v| v.max(0.0) as u32).unwrap_or(0),
        previous_outcome: parse_outcome(raw.previous_outcome.as_deref()),
        reflection: raw.reflection,
        thought: raw.thought,
        target: TargetDescriptor {
            description: raw.target.description,
            visible_text: raw.target.visible_text.filter(|v| !v.trim().is_empty()),
            control_type: raw.target.control_type.filter(|v| !v.trim().is_empty()),
        },
        expected_outcome: raw.expected_outcome,
        action,
        confidence: raw.confidence.unwrap_or(0.0) as f32,
        impact: parse_impact(raw.impact.as_deref()),
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RawVerification {
    verdict: String,
    reason: String,
    final_action: Option<LooseAction>,
    confidence: Option<f64>,
    grounding_evidence: Option<String>,
}

pub fn parse_verification(raw_text: &str) -> Result<VerifiedAction, String> {
    parse_verification_with(raw_text, &|id| Err(format!("Element {id} is not available; use x and y instead.")))
}

pub fn parse_verification_with(raw_text: &str, resolve: ElementResolver) -> Result<VerifiedAction, String> {
    let mut raw: RawVerification = parse_lenient(raw_text, "verification")?;
    if let Some(action) = raw.final_action.as_mut() {
        if !action.kind.trim().is_empty() {
            action.resolve_elements(resolve)?;
        }
    }
    let verdict = match raw.verdict.trim().to_lowercase().as_str() {
        "accept" | "accepted" | "approve" | "ok" => VerificationVerdict::Accept,
        "correct" | "corrected" => VerificationVerdict::Correct,
        "replan" => VerificationVerdict::Replan,
        "fail" | "reject" => VerificationVerdict::Fail,
        other => return Err(format!("Unknown verdict '{other}'. Allowed: accept, correct, replan, fail.")),
    };
    // An empty finalAction object (`{}`) means "none" for models that always fill the field.
    let final_action = match raw.final_action.filter(|a| !a.kind.trim().is_empty()) {
        Some(action) => match action.into_primitive() {
            Ok(primitive) => Some(primitive),
            Err(error) if verdict == VerificationVerdict::Correct => return Err(error),
            Err(_) => None,
        },
        None => None,
    };
    Ok(VerifiedAction {
        verdict,
        reason: raw.reason,
        final_action,
        confidence: raw.confidence.unwrap_or(0.0) as f32,
        grounding_evidence: raw.grounding_evidence.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_prose_fences_and_think_blocks() {
        let raw = "<think>I should {click}</think>\nSure!\n```json\n{\"a\": \"}\", \"b\": {\"c\": 1}}\n```\ntrailing";
        let extracted = extract_json_object(raw).unwrap();
        let value: Value = serde_json::from_str(extracted).unwrap();
        assert_eq!(value["a"], "}");
        assert_eq!(value["b"]["c"], 1);
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("{ unterminated").is_none());
    }

    #[test]
    fn tolerates_float_coordinates_and_unknown_fields() {
        let raw = r#"{"stepId":1.0,"previousOutcome":"not_applicable","reflection":"","thought":"open menu",
            "target":{"description":"File menu","visibleText":"File","extra":1},"expectedOutcome":"menu opens",
            "action":{"type":"click","x":640.4,"y":85.6,"unused":true},"confidence":0.9,"impact":"routine","bonus":"x"}"#;
        let proposal = parse_proposal(raw).unwrap();
        match proposal.action {
            ActionPrimitive::Click { x, y, clicks, .. } => assert_eq!((x, y, clicks), (640, 86, 1)),
            other => panic!("expected click, got {other:?}"),
        }
        assert_eq!(proposal.target.visible_text.as_deref(), Some("File"));
    }

    #[test]
    fn missing_fields_produce_specific_feedback() {
        let error = LooseAction { kind: "click".into(), x: Some(1.0), ..Default::default() }
            .into_primitive()
            .unwrap_err();
        assert!(error.contains("requires numeric x and y"));
        let error = LooseAction { kind: "hotkey".into(), ..Default::default() }.into_primitive().unwrap_err();
        assert!(error.contains("keys"));
        let error = LooseAction { kind: "teleport".into(), ..Default::default() }.into_primitive().unwrap_err();
        assert!(error.contains("Allowed"));
    }

    #[test]
    fn element_ids_resolve_to_the_element_centre() {
        let resolve = |id: u32| if id == 7 { Ok((250, 420)) } else { Err(format!("Element {id} does not exist.")) };
        let raw = r#"{"previousOutcome":"not_applicable","reflection":"r","thought":"t","target":{"description":"Save"},"expectedOutcome":"e","action":{"type":"click","element":7},"impact":"routine"}"#;
        let proposal = parse_proposal_with(raw, &resolve).unwrap();
        assert!(matches!(proposal.action, ActionPrimitive::Click { x: 250, y: 420, .. }));
        let bad = raw.replace("\"element\":7", "\"element\":99");
        assert!(parse_proposal_with(&bad, &resolve).unwrap_err().contains("does not exist"));
        // Without a resolver an element id is refused with advice to use x and y.
        assert!(parse_proposal(raw).unwrap_err().contains("not available"));
        let drag = r#"{"verdict":"correct","reason":"r","finalAction":{"type":"drag_and_drop","from_element":7,"to_element":7}}"#;
        assert!(matches!(parse_verification_with(drag, &resolve).unwrap().final_action, Some(ActionPrimitive::DragAndDrop { from_x: 250, to_y: 420, .. })));
    }

    #[test]
    fn window_and_url_actions_and_wait_seconds_parse() {
        let focus = LooseAction { kind: "switch_to".into(), name: Some("chrome".into()), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(focus, ActionPrimitive::FocusWindow { name } if name == "chrome"));
        let url = LooseAction { kind: "navigate".into(), url: Some("https://example.com".into()), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(url, ActionPrimitive::OpenUrl { url } if url == "https://example.com"));
        let wait = LooseAction { kind: "wait".into(), seconds: Some(2.5), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(wait, ActionPrimitive::WaitSettle { max_timeout_ms: 2500, .. }));
        assert!(LooseAction { kind: "open_url".into(), ..Default::default() }.into_primitive().is_err());
    }

    #[test]
    fn done_check_parses_and_defaults_to_not_complete() {
        let ok = parse_done_check(r#"{"evidence":"Notepad shows hello","complete":true,"missing":""}"#).unwrap();
        assert!(ok.complete);
        let sloppy = parse_done_check(r#"{"evidence":"unclear","extra":1}"#).unwrap();
        assert!(!sloppy.complete, "a reply without an explicit yes must not count as complete");
        assert!(parse_done_check("no json").is_err());
        assert_eq!(done_check_schema(0)["required"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn note_and_ask_user_actions_parse() {
        let note = LooseAction { kind: "remember".into(), text: Some("order #4411".into()), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(note, ActionPrimitive::Note { text } if text == "order #4411"));
        let ask = LooseAction { kind: "ask".into(), question: Some("Please solve the CAPTCHA".into()), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(ask, ActionPrimitive::AskUser { .. }));
        assert!(LooseAction { kind: "note".into(), ..Default::default() }.into_primitive().is_err());
    }

    #[test]
    fn element_numbers_are_bounded_and_only_offered_when_elements_exist() {
        let variants = |n: u32| proposal_schema(n)["properties"]["action"]["anyOf"].as_array().unwrap().clone();
        let has_element = |v: &Value| v["properties"].as_object().unwrap().keys().any(|k| k.contains("element"));
        assert!(variants(0).iter().all(|v| !has_element(v)), "no elements detected -> no element options");
        let with_elements = variants(7);
        let click = with_elements.iter().find(|v| v["properties"]["element"].is_object() && v["properties"]["type"]["enum"][0] == "click").unwrap();
        assert_eq!(click["properties"]["element"]["minimum"], 1);
        assert_eq!(click["properties"]["element"]["maximum"], 7);
        assert!(with_elements.iter().any(|v| v["properties"]["from_element"]["maximum"] == 7));
        assert!(SchemaSpec { name: "x", build: proposal_schema, max_element: 0 }.with_max_element(9).max_element == 9);
    }

    #[test]
    fn open_app_is_accepted_with_aliases() {
        let action = LooseAction { kind: "launch".into(), name: Some("notepad".into()), ..Default::default() }
            .into_primitive()
            .unwrap();
        assert!(matches!(action, ActionPrimitive::OpenApp { name } if name == "notepad"));
        let error = LooseAction { kind: "open_app".into(), ..Default::default() }.into_primitive().unwrap_err();
        assert!(error.contains("application name"));
    }

    #[test]
    fn a_cut_off_reply_is_not_mistaken_for_a_nested_object() {
        let truncated = r#"{"action": {"type": "click", "x": 1, "y": 2}, "thought": "I will search. Wait, I will search. Wait"#;
        assert!(extract_json_object(truncated).is_none());
        let error = parse_proposal(truncated).unwrap_err();
        assert!(error.contains("cut off"), "{error}");
    }

    #[test]
    fn schema_property_order_puts_reasoning_before_the_action() {
        let schema = proposal_schema(5);
        let keys: Vec<_> = schema["properties"].as_object().unwrap().keys().cloned().collect();
        let position = |name: &str| keys.iter().position(|k| k == name).unwrap();
        assert!(position("reflection") < position("action"));
        assert!(position("thought") < position("action"));
        assert!(position("target") < position("action"));
    }

    #[test]
    fn aliases_and_defaults() {
        let action = LooseAction { kind: "Right Click".into(), x: Some(5.0), y: Some(6.0), ..Default::default() }
            .into_primitive()
            .unwrap();
        assert!(matches!(action, ActionPrimitive::Click { button: MouseButton::Right, .. }));
        let action = LooseAction { kind: "done".into(), ..Default::default() }.into_primitive().unwrap();
        assert!(matches!(action, ActionPrimitive::Done { .. }));
        let action = LooseAction { kind: "key".into(), keys: Some(vec!["enter".into()]), ..Default::default() }
            .into_primitive()
            .unwrap();
        assert!(matches!(action, ActionPrimitive::KeyPress { key } if key == "enter"));
    }

    #[test]
    fn verification_accepts_empty_final_action_but_rejects_bad_correction() {
        let accepted = parse_verification(r#"{"verdict":"accept","reason":"ok","finalAction":{}}"#).unwrap();
        assert!(accepted.final_action.is_none());
        let bad = parse_verification(r#"{"verdict":"correct","reason":"r","finalAction":{"type":"click","x":1}}"#);
        assert!(bad.is_err());
        let good = parse_verification(r#"{"verdict":"correct","reason":"r","finalAction":{"type":"click","x":1,"y":2}}"#).unwrap();
        assert!(good.final_action.is_some());
    }

    #[test]
    fn schemas_are_valid_objects_with_required_keys() {
        for schema in [proposal_schema(5), verification_schema(5), final_outcome_schema(0)] {
            assert_eq!(schema["type"], "object");
            assert!(schema["required"].as_array().is_some_and(|r| !r.is_empty()));
        }
        let variants = proposal_schema(5)["properties"]["action"]["anyOf"].as_array().unwrap().clone();
        let covered: std::collections::BTreeSet<String> = variants
            .iter()
            .map(|v| v["properties"]["type"]["enum"][0].as_str().unwrap().to_owned())
            .collect();
        let expected: std::collections::BTreeSet<String> = ACTION_TYPES.iter().map(|t| t.to_string()).collect();
        assert_eq!(covered, expected, "every action type needs at least one schema variant");
        for variant in variants {
            assert_eq!(variant["additionalProperties"], false, "unknown field names must be rejected");
        }
        // A hotkey must carry its keys, a click its coordinates.
        let find = |kind: &str| proposal_schema(5)["properties"]["action"]["anyOf"].as_array().unwrap().iter().find(|v| v["properties"]["type"]["enum"][0] == kind).cloned().unwrap();
        assert!(find("hotkey")["required"].as_array().unwrap().iter().any(|f| f == "keys"));
        let clicks: Vec<Value> = proposal_schema(5)["properties"]["action"]["anyOf"].as_array().unwrap().iter().filter(|v| v["properties"]["type"]["enum"][0] == "click").cloned().collect();
        assert!(clicks.iter().any(|v| v["required"].as_array().unwrap().iter().any(|f| f == "x")), "a pixel click variant must exist");
        assert!(clicks.iter().any(|v| v["required"].as_array().unwrap().iter().any(|f| f == "element")), "an element click variant must exist");
    }

    #[test]
    fn shortcuts_written_as_one_string_are_split() {
        assert_eq!(split_key_combos(vec!["Ctrl+Shift+T".into()]), ["Ctrl", "Shift", "T"]);
        assert_eq!(split_key_combos(vec!["ctrl".into(), "+".into()]), ["ctrl", "+"]);
        assert_eq!(split_key_combos(vec!["ctrl + s".into()]), ["ctrl", "s"]);
        assert_eq!(split_key_combos(vec!["Ctrl++".into()]), ["Ctrl", "+"]);
        assert_eq!(split_key_combos(vec!["+".into()]), ["+"]);
        let hotkey = parse_lenient::<LooseAction>(r#"{"type":"hotkey","keys":["Ctrl+A"]}"#, "action").unwrap().into_primitive().unwrap();
        assert!(matches!(hotkey, ActionPrimitive::Hotkey { keys } if keys == ["Ctrl", "A"]));
        let press = parse_lenient::<LooseAction>(r#"{"type":"key_press","key":"ctrl+s"}"#, "action").unwrap().into_primitive().unwrap();
        assert!(matches!(press, ActionPrimitive::Hotkey { keys } if keys == ["ctrl", "s"]));
        let single = parse_lenient::<LooseAction>(r#"{"type":"key_press","key":"enter"}"#, "action").unwrap().into_primitive().unwrap();
        assert!(matches!(single, ActionPrimitive::KeyPress { key } if key == "enter"));
    }
}
