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

const ACTION_TYPES: &[&str] = &[
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
    // Added in Phase 9 (HARNESS_COMPLETION_PLAN.md §4).
    "triple_click",
    "mouse_down",
    "mouse_up",
    "draw_path",
    "key_down",
    "key_up",
    "wait",
    "zoom",
    "observe",
    "clipboard_read",
    "clipboard_write",
    "minimize_window",
    "maximize_window",
    "restore_window",
    "close_window",
    "move_window",
    "resize_window",
    "snap_window",
    "switch_desktop",
    "touch",
    "layout_windows",
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
    // Window id from the window list ("w3"): optional on pointer and typing actions.
    let window = json!({"type": "string", "maxLength": 12});
    let points = json!({"type": "array", "items": {"type": "array", "items": {"type": "integer"}}});

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
            "overwrite": {"type": "boolean"},
            "method": {"type": "string", "enum": ["auto", "keys", "unicode", "paste"]},
            "chars_per_second": {"type": "number"},
            "window": window.clone()
        }), &["text"]),
        action_variant("click", with(&with(&with(&with(&point, button.clone()), json!({"clicks": {"type": "integer"}})), hold.clone()), json!({"window": window.clone()})), &["x", "y"]),
        action_variant("double_click", with(&with(&point, button.clone()), json!({"window": window.clone()})), &["x", "y"]),
        action_variant("mouse_move", with(&point, json!({"dwell_ms": {"type": "integer"}, "window": window.clone()})), &["x", "y"]),
        action_variant("drag_and_drop", json!({
            "from_x": {"type": "integer"}, "from_y": {"type": "integer"},
            "to_x": {"type": "integer"}, "to_y": {"type": "integer"},
            "duration_ms": {"type": "integer"},
            "hold_keys": {"type": "array", "items": {"type": "string"}},
            "waypoints": points.clone(),
            "button": {"type": "string", "enum": ["left", "right", "middle"]},
            "speed": {"type": "string", "enum": ["slow", "normal", "fast"]},
            "window": window.clone()
        }), &["from_x", "from_y", "to_x", "to_y"]),
        action_variant("scroll", with(&point, json!({
            "clicks": {"type": "integer"},
            "horizontal": {"type": "boolean"},
            "hold_keys": {"type": "array", "items": {"type": "string"}},
            "delta": {"type": "integer"},
            "window": window.clone()
        })), &["clicks"]),
        action_variant("wait_settle", json!({"max_timeout_ms": {"type": "integer"}, "seconds": {"type": "number"}}), &[]),
        action_variant("note", json!({"text": {"type": "string", "maxLength": 300}}), &["text"]),
        action_variant("ask_user", json!({"question": {"type": "string", "maxLength": 240}}), &["question"]),
        // Added in Phase 9 (HARNESS_COMPLETION_PLAN.md §4).
        action_variant("triple_click", with(&with(&point, button.clone()), json!({"window": window.clone()})), &["x", "y"]),
        action_variant("mouse_down", with(&point, button.clone()), &["x", "y"]),
        action_variant("mouse_up", with(&point, button.clone()), &[]),
        action_variant("draw_path", json!({
            "points": points.clone(),
            "shape": {"type": "object"},
            "closed": {"type": "boolean"},
            "duration_ms": {"type": "integer"},
            "hold_keys": {"type": "array", "items": {"type": "string"}},
            "button": {"type": "string", "enum": ["left", "right", "middle"]},
            "pointer": {"type": "string", "enum": ["mouse", "pen", "touch"]},
            "pressure": {"type": "number"},
            "tilt_x": {"type": "integer"},
            "tilt_y": {"type": "integer"}
        }), &[]),
        action_variant("key_down", json!({"key": {"type": "string"}}), &["key"]),
        action_variant("key_up", json!({"key": {"type": "string"}}), &["key"]),
        action_variant("wait", json!({"seconds": {"type": "number"}}), &["seconds"]),
        action_variant("zoom", with(&point, json!({"width": {"type": "integer"}, "height": {"type": "integer"}})), &["x", "y", "width", "height"]),
        action_variant("observe", json!({}), &[]),
        action_variant("clipboard_read", json!({}), &[]),
        action_variant("clipboard_write", json!({"text": {"type": "string"}}), &["text"]),
        action_variant("minimize_window", json!({"window": window.clone()}), &[]),
        action_variant("maximize_window", json!({"window": window.clone()}), &[]),
        action_variant("restore_window", json!({"window": window.clone()}), &[]),
        action_variant("close_window", json!({"window": window.clone()}), &[]),
        action_variant("move_window", with(&point, json!({"window": window.clone()})), &["x", "y"]),
        action_variant("resize_window", json!({"window": window.clone(), "width": {"type": "integer"}, "height": {"type": "integer"}}), &["width", "height"]),
        action_variant("snap_window", json!({"window": window.clone(), "side": {"type": "string", "enum": ["left", "right", "top", "bottom", "top_left", "top_right", "bottom_left", "bottom_right"]}}), &["side"]),
        action_variant("layout_windows", json!({"layout": {"type": "string", "enum": ["grid", "columns", "rows"]}, "windows": {"type": "array", "items": {"type": "string"}}}), &["windows"]),
        action_variant("switch_desktop", json!({"direction": {"type": "string", "enum": ["left", "right"]}}), &["direction"]),
        action_variant("touch", with(&point, json!({
            "gesture": {"type": "string", "enum": ["tap", "double_tap", "long_press", "swipe", "pinch", "rotate"]},
            "to_x": {"type": "integer"}, "to_y": {"type": "integer"},
            "scale": {"type": "number"}, "angle": {"type": "number"}, "duration_ms": {"type": "integer"}
        })), &["gesture", "x", "y"]),
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
            "impact": {"type": "string", "enum": ["routine", "destructive", "financial", "credential", "communication"]},
            "voice_command": {"type": "string", "maxLength": 80}
        },
        // voice_command is required here so constrained decoders always produce it; the parser
        // still accepts a reply without it (backward compatible, HARNESS_COMPLETION_PLAN.md D9.1).
        "required": ["previousOutcome", "reflection", "thought", "target", "expectedOutcome", "action", "impact", "voice_command"],
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

/// Grounding mode B: the second call answers only "where is this target": an element number or x/y.
pub fn grounding_schema(max_element: u32) -> Value {
    let element = if max_element > 0 { json!({"type": "integer", "minimum": 1, "maximum": max_element}) } else { json!({"type": "integer", "minimum": 1}) };
    json!({
        "type": "object",
        "properties": {
            "element": element,
            "x": {"type": "integer"},
            "y": {"type": "integer"},
            "found": {"type": "boolean"}
        },
        "required": ["found"],
        "additionalProperties": false
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawGrounding {
    element: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
    found: Option<bool>,
}

/// Where the grounding call says the target is (in screenshot pixels).
pub fn parse_grounding_with(raw_text: &str, resolve: ElementResolver) -> Result<(i32, i32), String> {
    let raw: RawGrounding = parse_lenient(raw_text, "grounding answer")?;
    if raw.found == Some(false) {
        return Err("The grounding call could not find the target on screen.".to_owned());
    }
    if let Some(id) = raw.element.filter(|v| *v >= 1.0) {
        return resolve(id as u32);
    }
    match (raw.x, raw.y) {
        (Some(x), Some(y)) => Ok((x.round() as i32, y.round() as i32)),
        _ => Err("The grounding answer had neither an element nor x and y.".to_owned()),
    }
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
pub const GROUNDING: SchemaSpec = SchemaSpec { name: "aura_grounding", build: grounding_schema, max_element: 0 };
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

/// Numbers out of whatever a model wrote for a point: `[x, y]`, `"(x, y)"`, `"x,y"`,
/// `"<point>x y</point>"`, `{"x":..,"y":..}`. Four numbers are a box `[x1, y1, x2, y2]`.
fn numbers_in(value: &Value) -> Vec<f64> {
    match value {
        Value::Array(items) => items.iter().flat_map(numbers_in).collect(),
        Value::Number(n) => n.as_f64().into_iter().collect(),
        Value::Object(map) => ["x", "y"].iter().filter_map(|k| map.get(*k)).flat_map(numbers_in).collect(),
        Value::String(text) => text
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|t| !t.is_empty() && *t != "-" && *t != ".")
            .filter_map(|t| t.parse::<f64>().ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// A point from 2 numbers, or the centre of a box from 4.
fn point_of(value: &Value) -> Option<(f64, f64)> {
    match numbers_in(value).as_slice() {
        [x, y] => Some((*x, *y)),
        [x1, y1, x2, y2] => Some(((x1 + x2) / 2.0, (y1 + y2) / 2.0)),
        _ => None,
    }
}

/// Rewrites the action-object dialects that vision models commonly use into this schema's
/// fields, so an order that is clear in intent is executed instead of rejected:
/// `{"action": "click"}` for `type`, `coordinate`/`point`/`position`/`start_box`/`bbox` for x/y,
/// `start_*`/`end_*`/`from`/`to` for drags, `"keys": "ctrl+a"` for a key list, `content`/`value`
/// for typed text, `"element": "#7"` or `"[7]"` for a number. Fields already in the schema win.
pub fn normalize_action_dialect(value: &mut Value) {
    let Value::Object(map) = value else { return };
    if !map.contains_key("type") {
        for alias in ["action", "action_type", "actionType", "kind", "op"] {
            if let Some(Value::String(name)) = map.get(alias).cloned() {
                map.insert("type".into(), Value::String(name));
                break;
            }
        }
    }
    if let Some(Value::String(kind)) = map.get("type").cloned() {
        let lowered = kind.trim().to_lowercase().replace([' ', '-'], "_");
        map.insert("type".into(), Value::String(lowered));
    }
    if !(map.contains_key("x") && map.contains_key("y")) {
        for alias in ["coordinate", "coordinates", "point", "position", "pos", "coords", "start_box", "click_point", "location", "xy", "bbox", "box", "bounding_box", "target_point"] {
            if let Some((x, y)) = map.get(alias).and_then(point_of) {
                map.insert("x".into(), Value::from(x));
                map.insert("y".into(), Value::from(y));
                break;
            }
        }
    }
    if !(map.contains_key("from_x") && map.contains_key("to_x")) {
        let from = ["start_coordinate", "start", "from", "start_point", "start_box", "source"].iter().find_map(|k| map.get(*k).and_then(point_of));
        let to = ["end_coordinate", "end", "to", "end_point", "end_box", "target", "destination"].iter().find_map(|k| map.get(*k).and_then(point_of));
        if let (Some(from), Some(to)) = (from, to) {
            map.insert("from_x".into(), Value::from(from.0));
            map.insert("from_y".into(), Value::from(from.1));
            map.insert("to_x".into(), Value::from(to.0));
            map.insert("to_y".into(), Value::from(to.1));
        }
    }
    // "keys": "ctrl+a" or "ctrl a"; "key": "ctrl+s" on a hotkey.
    if let Some(Value::String(text)) = map.get("keys").cloned() {
        let keys: Vec<Value> = text.split(['+', ' ', ',']).map(str::trim).filter(|k| !k.is_empty()).map(|k| Value::String(k.to_owned())).collect();
        map.insert("keys".into(), Value::Array(keys));
    }
    let kind = map.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
    if !map.contains_key("keys") {
        if let Some(Value::String(text)) = map.get("key").cloned() {
            if text.contains('+') && text.trim().len() > 1 {
                let keys: Vec<Value> = text.split('+').map(str::trim).filter(|k| !k.is_empty()).map(|k| Value::String(k.to_owned())).collect();
                map.insert("keys".into(), Value::Array(keys));
                if kind == "key_press" || kind == "key" || kind == "press" {
                    map.insert("type".into(), Value::String("hotkey".into()));
                }
            }
        }
    }
    if !map.contains_key("text") {
        for alias in ["content", "value", "input", "string", "text_to_type"] {
            if let Some(Value::String(text)) = map.get(alias).cloned() {
                map.insert("text".into(), Value::String(text));
                break;
            }
        }
    }
    for key in ["element", "from_element", "to_element"] {
        if let Some(Value::String(text)) = map.get(key).cloned() {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            if let Ok(number) = digits.parse::<u32>() {
                map.insert(key.into(), Value::from(number));
            }
        }
    }
    // Remove dialect-only keys so strict deserialisation does not trip over them.
    for alias in [
        "action", "action_type", "actionType", "op", "kind", "coordinate", "coordinates", "point", "position", "pos", "coords", "start_box", "click_point", "location", "xy",
        "bbox", "box", "bounding_box", "target_point", "start_coordinate", "end_coordinate", "start", "end", "from", "to", "start_point", "end_point", "end_box",
        "source", "destination", "content", "value", "input", "string", "text_to_type",
    ] {
        map.remove(alias);
    }
}

/// The proposal-level dialects: a list of actions (`"actions": [...]`, the first is taken), or the
/// action fields written at the top level instead of inside `"action"`.
fn normalize_proposal_dialect(value: &mut Value) {
    let Value::Object(map) = value else { return };
    if !map.contains_key("action") {
        if let Some(Value::Array(list)) = map.get("actions").cloned() {
            if let Some(first) = list.into_iter().next() {
                map.insert("action".into(), first);
            }
        } else if map.contains_key("type") || map.contains_key("action_type") {
            let mut action = serde_json::Map::new();
            for key in map.keys().cloned().collect::<Vec<_>>() {
                if !["previousOutcome", "reflection", "thought", "target", "expectedOutcome", "confidence", "impact", "voice_command", "voiceCommand", "stepId"].contains(&key.as_str()) {
                    if let Some(v) = map.remove(&key) {
                        action.insert(key, v);
                    }
                }
            }
            map.insert("action".into(), Value::Object(action));
        }
    } else if let Some(Value::String(name)) = map.get("action").cloned() {
        // {"action": "click", "coordinate": [..]} at the top level.
        let mut action = serde_json::Map::new();
        action.insert("type".into(), Value::String(name));
        for key in map.keys().cloned().collect::<Vec<_>>() {
            if !["previousOutcome", "reflection", "thought", "target", "expectedOutcome", "confidence", "impact", "voice_command", "voiceCommand", "stepId", "action"].contains(&key.as_str()) {
                if let Some(v) = map.remove(&key) {
                    action.insert(key, v);
                }
            }
        }
        map.insert("action".into(), Value::Object(action));
    }
    map.remove("actions");
    if let Some(action) = map.get_mut("action") {
        normalize_action_dialect(action);
    }
    // `target` given as plain text.
    if let Some(Value::String(text)) = map.get("target").cloned() {
        map.insert("target".into(), serde_json::json!({"description": text, "visibleText": text}));
    }
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
    let mut value: Value = serde_json::from_str(json_text).map_err(|error| format!("Your {label} JSON had the wrong shape: {error}"))?;
    if label == "action proposal" {
        normalize_proposal_dialect(&mut value);
    }
    if let Some(action) = value.get_mut("final_action").or_else(|| None) {
        normalize_action_dialect(action);
    }
    coerce_scalars(&mut value);
    serde_json::from_value(value).map_err(|error| format!("Your {label} JSON had the wrong shape: {error}"))
}

/// Keys that hold numbers and booleans in every model schema. Small models sometimes quote them
/// (`"x": "640"`, `"press_enter": "true"`); those are converted before typed parsing. Only these
/// keys are touched, so text fields are never changed. A window id given as a number becomes text.
const NUMBER_KEYS: &[&str] = &[
    "x", "y", "clicks", "from_x", "from_y", "to_x", "to_y", "duration_ms", "seconds", "element", "from_element", "to_element",
    "max_timeout_ms", "dwell_ms", "hold_ms", "delta", "chars_per_second", "pressure", "tilt_x", "tilt_y", "width", "height",
    "scale", "angle", "confidence", "stepId", "subgoal_index",
];
const BOOL_KEYS: &[&str] = &["press_enter", "overwrite", "horizontal", "closed", "complete", "completed"];

pub fn coerce_scalars(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                match item {
                    Value::String(text) if NUMBER_KEYS.contains(&key.as_str()) => {
                        if let Some(number) = text.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
                            *item = Value::Number(number);
                        }
                    }
                    Value::String(text) if BOOL_KEYS.contains(&key.as_str()) => match text.trim().to_lowercase().as_str() {
                        "true" | "yes" | "1" => *item = Value::Bool(true),
                        "false" | "no" | "0" => *item = Value::Bool(false),
                        _ => {}
                    },
                    Value::Number(number) if key == "window" => *item = Value::String(format!("w{number}")),
                    Value::Array(list) if key == "windows" => {
                        for entry in list.iter_mut() {
                            if let Value::Number(number) = entry {
                                *entry = Value::String(format!("w{number}"));
                            }
                        }
                    }
                    _ => coerce_scalars(item),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(coerce_scalars),
        _ => {}
    }
}

/// `[[x, y], ...]` from loose JSON (numbers may be quoted), at least `min` points.
fn point_list(value: &Value, what: &str, min: usize) -> Result<Vec<(i32, i32)>, String> {
    let points: Option<Vec<(i32, i32)>> = value.as_array().map(|items| {
        items
            .iter()
            .filter_map(|p| {
                let (x, y) = match p {
                    Value::Array(pair) => (pair.first()?, pair.get(1)?),
                    Value::Object(map) => (map.get("x")?, map.get("y")?),
                    _ => return None,
                };
                Some((to_i32(crate::input::shapes::lenient_number(x)?), to_i32(crate::input::shapes::lenient_number(y)?)))
            })
            .collect()
    });
    match points {
        Some(points) if points.len() >= min && Some(points.len()) == value.as_array().map(Vec::len) => Ok(points),
        _ => Err(format!("{what} must be a list of at least {min} [x, y] points in screenshot pixels.")),
    }
}

/// Closest allowed action name, for the "unknown action" feedback.
fn nearest_action_name(name: &str) -> Option<&'static str> {
    fn distance(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut previous = row[0];
            row[0] = i;
            for j in 1..=b.len() {
                let current = row[j];
                row[j] = (row[j] + 1).min(row[j - 1] + 1).min(previous + usize::from(a[i - 1] != b[j - 1]));
                previous = current;
            }
        }
        row[b.len()]
    }
    ACTION_TYPES.iter().map(|candidate| (distance(name, candidate), *candidate)).filter(|(d, c)| *d <= 3.max(c.len() / 3)).min().map(|(_, c)| c)
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
    // Optional fields added in Phase 9 (HARNESS_COMPLETION_PLAN.md §4).
    pub dwell_ms: Option<f64>,
    pub hold_ms: Option<f64>,
    pub delta: Option<f64>,
    pub method: Option<String>,
    pub chars_per_second: Option<f64>,
    pub waypoints: Option<Value>,
    pub points: Option<Value>,
    pub shape: Option<Value>,
    pub closed: Option<bool>,
    pub pointer: Option<String>,
    pub pressure: Option<f64>,
    pub tilt_x: Option<f64>,
    pub tilt_y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub window: Option<String>,
    pub side: Option<String>,
    pub command: Option<String>,
    pub direction: Option<String>,
    pub gesture: Option<String>,
    pub scale: Option<f64>,
    pub angle: Option<f64>,
    pub layout: Option<String>,
    pub windows: Option<Vec<String>>,
    pub speed: Option<String>,
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

        // Optional fields on an existing action name turn into the richer variant (Phase 9).
        let dwell_ms = self.dwell_ms.map(|d| d.round().clamp(0.0, 10_000.0) as u64);
        let duration_from_speed = match self.speed.as_deref().map(|s| s.trim().to_lowercase()) {
            Some(s) if s == "slow" => Some(1200),
            Some(s) if s == "fast" => Some(150),
            _ => None,
        };
        match kind.as_str() {
            "click" | "left_click" | "right_click" | "middle_click" | "triple_click" => {
                let (x, y) = need_point(self.x, self.y, "click")?;
                let button = match kind.as_str() {
                    "right_click" => MouseButton::Right,
                    "middle_click" => MouseButton::Middle,
                    _ => button,
                };
                let clicks = if kind == "triple_click" { 3 } else { self.clicks.map(|c| c.round().clamp(1.0, 3.0) as u32).unwrap_or(1) };
                Ok(ActionPrimitive::Click { x, y, button, clicks, hold_keys })
            }
            "mouse_move" | "move" | "hover" | "move_mouse" | "mouse_hover" => {
                let (x, y) = need_point(self.x, self.y, "mouse_move")?;
                Ok(match dwell_ms {
                    Some(dwell_ms) => ActionPrimitive::Hover { x, y, dwell_ms },
                    None => ActionPrimitive::MouseMove { x, y },
                })
            }
            "mouse_down" | "press_mouse" | "mouse_press" => {
                let (x, y) = need_point(self.x, self.y, "mouse_down")?;
                Ok(ActionPrimitive::MouseDown { x, y, button })
            }
            "mouse_up" | "release_mouse" | "mouse_release" => {
                let (x, y) = match (self.x, self.y) {
                    (Some(x), Some(y)) => (Some(to_i32(x)), Some(to_i32(y))),
                    _ => (None, None),
                };
                Ok(ActionPrimitive::MouseUp { x, y, button })
            }
            "draw_path" | "draw" | "stroke" | "draw_shape" => {
                let mut points = match (&self.points, &self.shape) {
                    (Some(points), _) => point_list(points, "draw_path points", 1)?,
                    (None, Some(shape)) => {
                        let shape = crate::input::shapes::Shape::from_json(shape)?;
                        let mut points = crate::input::shapes::to_pixels(&shape.outline());
                        if shape.is_closed() && points.first() != points.last() {
                            points.push(points[0]);
                        }
                        points
                    }
                    (None, None) => return Err("draw_path needs 'points' [[x,y],...] or a 'shape' {kind: circle|ellipse|spiral|bezier|polyline|line|sine, ...}.".into()),
                };
                if self.closed.unwrap_or(false) && points.len() > 2 && points.first() != points.last() {
                    points.push(points[0]);
                }
                let pointer = match self.pointer.as_deref().map(|p| p.trim().to_lowercase()) {
                    None => crate::input::PointerKind::Mouse,
                    Some(p) if p == "mouse" || p.is_empty() => crate::input::PointerKind::Mouse,
                    Some(p) if p == "pen" || p == "stylus" => crate::input::PointerKind::Pen,
                    Some(p) if p == "touch" || p == "finger" => crate::input::PointerKind::Touch,
                    Some(other) => return Err(format!("Unknown pointer '{other}'. Use mouse, pen or touch.")),
                };
                Ok(ActionPrimitive::DrawPath {
                    points,
                    button,
                    duration_ms: self.duration_ms.or(duration_from_speed.map(f64::from)).map(|d| d.round().clamp(0.0, 60_000.0) as u64).unwrap_or(0),
                    hold_keys,
                    pointer,
                    pressure: self.pressure.map(|p| p.clamp(0.0, 1.0) as f32),
                    tilt_x: self.tilt_x.map(to_i32),
                    tilt_y: self.tilt_y.map(to_i32),
                })
            }
            "key_down" | "hold_key_down" | "press_key_down" => {
                let key = self.key.or_else(|| self.keys.and_then(|k| k.into_iter().next())).filter(|k| !k.trim().is_empty()).ok_or("key_down requires a 'key'.")?;
                Ok(ActionPrimitive::KeyDown { key })
            }
            "key_up" | "release_key" => {
                let key = self.key.or_else(|| self.keys.and_then(|k| k.into_iter().next())).filter(|k| !k.trim().is_empty()).ok_or("key_up requires a 'key'.")?;
                Ok(ActionPrimitive::KeyUp { key })
            }
            "zoom" | "zoom_in" | "look_closer" => {
                let (x, y) = need_point(self.x, self.y, "zoom")?;
                let (width, height) = match (self.width, self.height) {
                    (Some(w), Some(h)) if w >= 8.0 && h >= 8.0 => (to_i32(w), to_i32(h)),
                    _ => return Err("zoom requires x, y (top-left of the region) and width, height (at least 8 pixels each).".into()),
                };
                Ok(ActionPrimitive::Zoom { x, y, width, height })
            }
            "observe" | "look" | "screenshot" | "refresh" => Ok(ActionPrimitive::Observe {}),
            "clipboard_read" | "read_clipboard" | "get_clipboard" => Ok(ActionPrimitive::ClipboardRead {}),
            "clipboard_write" | "set_clipboard" | "write_clipboard" | "copy_text" => {
                let text = self.text.ok_or("clipboard_write requires the 'text' to put on the clipboard.")?;
                Ok(ActionPrimitive::ClipboardWrite { text })
            }
            "window_action" | "minimize_window" | "maximize_window" | "restore_window" | "close_window" | "move_window" | "resize_window" | "snap_window" | "minimize" | "maximize" => {
                use crate::input::WindowCommand as C;
                let command_name = if kind == "window_action" { self.command.clone().unwrap_or_default().trim().to_lowercase() } else { kind.trim_end_matches("_window").to_owned() };
                let command = match command_name.as_str() {
                    "minimize" => C::Minimize,
                    "maximize" => C::Maximize,
                    "restore" => C::Restore,
                    "close" => C::Close,
                    "move" => C::Move,
                    "resize" => C::Resize,
                    "snap" => C::Snap,
                    other => return Err(format!("Unknown window command '{other}'. Use minimize, maximize, restore, close, move, resize or snap.")),
                };
                Ok(ActionPrimitive::WindowAction {
                    command,
                    window: self.window.or(self.name).filter(|w| !w.trim().is_empty()),
                    x: self.x.map(to_i32),
                    y: self.y.map(to_i32),
                    width: self.width.map(to_i32),
                    height: self.height.map(to_i32),
                    side: self.side.or(self.direction),
                })
            }
            "switch_desktop" | "virtual_desktop" => {
                let direction = self.direction.or(self.side).filter(|d| !d.trim().is_empty()).ok_or("switch_desktop requires a 'direction': left or right.")?;
                Ok(ActionPrimitive::SwitchDesktop { direction })
            }
            "touch" | "tap" | "double_tap" | "long_press" | "swipe" | "pinch" | "rotate" => {
                use crate::input::TouchGesture as G;
                let gesture_name = if kind == "touch" { self.gesture.clone().unwrap_or_else(|| "tap".into()).trim().to_lowercase().replace([' ', '-'], "_") } else { kind.clone() };
                let gesture = match gesture_name.as_str() {
                    "tap" => G::Tap,
                    "double_tap" => G::DoubleTap,
                    "long_press" | "press_and_hold" => G::LongPress,
                    "swipe" => G::Swipe,
                    "pinch" | "zoom" => G::Pinch,
                    "rotate" => G::Rotate,
                    other => return Err(format!("Unknown touch gesture '{other}'. Use tap, double_tap, long_press, swipe, pinch or rotate.")),
                };
                let (x, y) = need_point(self.x, self.y, "touch")?;
                if gesture == G::Swipe && (self.to_x.is_none() || self.to_y.is_none()) {
                    return Err("a swipe requires to_x and to_y.".into());
                }
                Ok(ActionPrimitive::Touch {
                    gesture,
                    x,
                    y,
                    to_x: self.to_x.map(to_i32),
                    to_y: self.to_y.map(to_i32),
                    scale: self.scale.map(|s| s as f32),
                    angle: self.angle.map(|a| a as f32),
                    duration_ms: self.duration_ms.map(|d| d.round().clamp(0.0, 10_000.0) as u64).unwrap_or(0),
                })
            }
            "layout_windows" | "tile_windows" | "arrange_windows" => {
                let windows = self.windows.filter(|w| !w.is_empty()).ok_or("layout_windows requires 'windows': a list of window ids such as [\"w1\", \"w3\"].")?;
                Ok(ActionPrimitive::LayoutWindows { layout: self.layout.unwrap_or_else(|| "grid".into()), windows })
            }
            "double_click" | "doubleclick" => {
                let (x, y) = need_point(self.x, self.y, "double_click")?;
                Ok(ActionPrimitive::DoubleClick { x, y, button })
            }
            "drag_and_drop" | "drag" | "drag_drop" => {
                let (from_x, from_y) = need_point(self.from_x, self.from_y, "drag_and_drop start (from_x, from_y)")?;
                let (to_x, to_y) = need_point(self.to_x, self.to_y, "drag_and_drop end (to_x, to_y)")?;
                // Waypoints, a non-left button or a speed make it a stroke through those points.
                let waypoints = match &self.waypoints {
                    Some(value) => point_list(value, "drag_and_drop waypoints", 0)?,
                    None => Vec::new(),
                };
                if !waypoints.is_empty() || button != MouseButton::Left || duration_from_speed.is_some() {
                    let mut points = vec![(from_x, from_y)];
                    points.extend(waypoints);
                    points.push((to_x, to_y));
                    return Ok(ActionPrimitive::DrawPath {
                        points,
                        button,
                        duration_ms: self.duration_ms.or(duration_from_speed.map(f64::from)).map(|d| d.round().clamp(100.0, 20_000.0) as u64).unwrap_or(0),
                        hold_keys,
                        pointer: crate::input::PointerKind::Mouse,
                        pressure: None,
                        tilt_x: None,
                        tilt_y: None,
                    });
                }
                let duration_ms = self.duration_ms.map(|d| d.round().clamp(100.0, 5000.0) as u64).unwrap_or(250);
                Ok(ActionPrimitive::DragAndDrop { from_x, from_y, to_x, to_y, duration_ms, hold_keys })
            }
            "type_text" | "type" | "text" | "write" => {
                let text = self.text.ok_or("type_text requires a 'text' string.")?;
                let method = match self.method.as_deref().map(|m| m.trim().to_lowercase()) {
                    None => None,
                    Some(m) if m == "auto" || m.is_empty() => Some(crate::input::TypeMethod::Auto),
                    Some(m) if m == "keys" || m == "keyboard" => Some(crate::input::TypeMethod::Keys),
                    Some(m) if m == "unicode" => Some(crate::input::TypeMethod::Unicode),
                    Some(m) if m == "paste" || m == "clipboard" => Some(crate::input::TypeMethod::Paste),
                    Some(other) => return Err(format!("Unknown typing method '{other}'. Use auto, keys, unicode or paste.")),
                };
                if method.is_some() || self.chars_per_second.is_some() {
                    return Ok(ActionPrimitive::TypeWith {
                        text,
                        press_enter: self.press_enter.unwrap_or(false),
                        overwrite: self.overwrite.unwrap_or(false),
                        method: method.unwrap_or_default(),
                        chars_per_second: self.chars_per_second.map(|c| c as f32),
                    });
                }
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
                if let Some(hold_ms) = self.hold_ms {
                    return Ok(ActionPrimitive::HoldKey { key, hold_ms: hold_ms.round().clamp(0.0, 30_000.0) as u64 });
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
                let (x, y) = match (self.x, self.y) {
                    (Some(x), Some(y)) => (Some(to_i32(x)), Some(to_i32(y))),
                    (None, None) => (None, None),
                    _ => return Err("scroll needs both x and y, or neither.".to_owned()),
                };
                // Keys held (Ctrl = zoom) or a raw wheel delta make it a fine scroll.
                if !hold_keys.is_empty() || self.delta.is_some() {
                    let delta = match (self.delta, self.clicks) {
                        (Some(delta), _) => to_i32(delta),
                        (None, Some(clicks)) => to_i32(clicks) * 120,
                        (None, None) => return Err("scroll requires 'clicks' (positive up, negative down) or a wheel 'delta'.".into()),
                    };
                    return Ok(ActionPrimitive::ScrollFine { x, y, delta: delta.clamp(-24_000, 24_000), horizontal: self.horizontal.unwrap_or(false), hold_keys });
                }
                let clicks = self.clicks.ok_or("scroll requires an integer 'clicks' (positive up, negative down).")?;
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
                "Unknown action type '{other}'.{} Allowed: {}.",
                nearest_action_name(other).map(|n| format!(" Did you mean '{n}'?")).unwrap_or_default(),
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
    #[serde(rename = "voice_command", alias = "voiceCommand")]
    voice_command: Option<String>,
}

pub fn parse_proposal(raw_text: &str) -> Result<ActionProposal, String> {
    parse_proposal_with(raw_text, &|id| Err(format!("Element {id} is not available; use x and y instead.")))
}

pub fn parse_proposal_with(raw_text: &str, resolve: ElementResolver) -> Result<ActionProposal, String> {
    let mut raw: RawProposal = parse_lenient(raw_text, "action proposal")?;
    raw.action.resolve_elements(resolve)?;
    // `window` on a pointer or typing action names the window it is meant for; on a window
    // command it is the window to change (consumed by the command itself).
    let target_window = raw.action.window.clone().filter(|_| !matches!(raw.action.kind.trim(), "window_action" | "layout_windows") && !raw.action.kind.ends_with("_window"));
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
        voice_command: raw.voice_command.map(|v| v.trim().chars().take(120).collect::<String>()).filter(|v| !v.is_empty()),
        target_window,
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

    fn contract_tail() -> &'static str {
        r#""previousOutcome":"not_applicable","reflection":"r","thought":"t","target":{"description":"d"},"expectedOutcome":"e","impact":"routine""#
    }

    fn proposal_text(action: &str, extra: &str) -> String {
        format!("{{{},\"action\":{action}{extra}}}", contract_tail())
    }

    #[test]
    fn voice_command_is_read_when_present_and_optional_when_missing() {
        let with = parse_proposal(&proposal_text(r#"{"type":"hotkey","keys":["ctrl","s"]}"#, r#","voice_command":"Saving the file""#)).unwrap();
        assert_eq!(with.voice_command.as_deref(), Some("Saving the file"));
        let camel = parse_proposal(&proposal_text(r#"{"type":"hotkey","keys":["ctrl","s"]}"#, r#","voiceCommand":"Saving""#)).unwrap();
        assert_eq!(camel.voice_command.as_deref(), Some("Saving"));
        let without = parse_proposal(&proposal_text(r#"{"type":"hotkey","keys":["ctrl","s"]}"#, "")).unwrap();
        assert_eq!(without.voice_command, None, "an old-style reply still parses");
        let schema = proposal_schema(0);
        assert!(schema["required"].as_array().unwrap().iter().any(|k| k == "voice_command"));
        assert_eq!(schema["properties"]["voice_command"]["type"], "string");
    }

    #[test]
    fn fenced_prose_wrapped_and_quoted_number_replies_parse() {
        let core = proposal_text(r#"{"type":"click","x":"640","y":"360.4","clicks":"2"}"#, "");
        for wrapped in [format!("```json\n{core}\n```"), format!("Sure! Here is my answer:\n{core}\nHope that helps."), format!("<think>maybe {{x}}</think>{core}")] {
            match parse_proposal(&wrapped).unwrap().action {
                ActionPrimitive::Click { x, y, clicks, .. } => assert_eq!((x, y, clicks), (640, 360, 2)),
                other => panic!("expected click, got {other:?}"),
            }
        }
        let quoted_bool = parse_proposal(&proposal_text(r#"{"type":"type_text","text":"hi","press_enter":"true"}"#, "")).unwrap();
        assert!(matches!(quoted_bool.action, ActionPrimitive::TypeText { press_enter: true, .. }));
    }

    #[test]
    fn empty_truncated_wrong_typed_and_unknown_replies_give_specific_feedback() {
        assert!(parse_proposal("").unwrap_err().contains("did not contain a JSON object"));
        assert!(parse_proposal("   I think I should click the button.").unwrap_err().contains("did not contain"));
        let cut = proposal_text(r#"{"type":"click","x":10,"y":10}"#, "");
        assert!(parse_proposal(&cut[..cut.len() / 2]).unwrap_err().contains("cut off"));
        assert!(parse_proposal(&proposal_text(r#"{"type":"click","x":"left","y":10}"#, "")).is_err(), "a word where a number belongs is refused");
        let unknown = parse_proposal(&proposal_text(r#"{"type":"duble_click","x":1,"y":2}"#, "")).unwrap_err();
        assert!(unknown.contains("Did you mean 'double_click'"), "{unknown}");
        assert!(parse_proposal(&proposal_text(r#"{"x":1,"y":2}"#, "")).unwrap_err().contains("no 'type'"));
        // An extremely long reply is not a crash: the object is found or a clear error is given.
        let huge = format!("{}{}", "noise ".repeat(40_000), proposal_text(r#"{"type":"observe"}"#, ""));
        assert!(matches!(parse_proposal(&huge).unwrap().action, ActionPrimitive::Observe {}));
    }

    #[test]
    fn new_actions_and_optional_fields_parse_into_their_variants() {
        let parse = |action: &str| parse_proposal(&proposal_text(action, "")).map(|p| p.action);
        assert!(matches!(parse(r#"{"type":"triple_click","x":5,"y":6}"#).unwrap(), ActionPrimitive::Click { clicks: 3, .. }));
        assert!(matches!(parse(r#"{"type":"mouse_move","x":5,"y":6,"dwell_ms":800}"#).unwrap(), ActionPrimitive::Hover { dwell_ms: 800, .. }));
        assert!(matches!(parse(r#"{"type":"mouse_move","x":5,"y":6}"#).unwrap(), ActionPrimitive::MouseMove { .. }), "unchanged without the new field");
        assert!(matches!(parse(r#"{"type":"mouse_down","x":5,"y":6,"button":"right"}"#).unwrap(), ActionPrimitive::MouseDown { button: MouseButton::Right, .. }));
        assert!(matches!(parse(r#"{"type":"mouse_up"}"#).unwrap(), ActionPrimitive::MouseUp { x: None, .. }));
        match parse(r#"{"type":"drag_and_drop","from_x":1,"from_y":1,"to_x":50,"to_y":1,"waypoints":[[20,30],["30","30"]]}"#).unwrap() {
            ActionPrimitive::DrawPath { points, .. } => assert_eq!(points, vec![(1, 1), (20, 30), (30, 30), (50, 1)]),
            other => panic!("expected a stroke, got {other:?}"),
        }
        assert!(matches!(parse(r#"{"type":"drag_and_drop","from_x":1,"from_y":1,"to_x":50,"to_y":1}"#).unwrap(), ActionPrimitive::DragAndDrop { .. }));
        match parse(r#"{"type":"draw_path","shape":{"kind":"circle","cx":100,"cy":100,"r":20},"pointer":"pen","pressure":0.7}"#).unwrap() {
            ActionPrimitive::DrawPath { points, pointer, pressure, .. } => {
                assert_eq!(pointer, crate::input::PointerKind::Pen);
                assert_eq!(pressure, Some(0.7));
                assert_eq!(points.first(), points.last(), "a circle is closed");
                assert!(points.len() > 60);
            }
            other => panic!("expected a stroke, got {other:?}"),
        }
        assert!(parse(r#"{"type":"draw_path"}"#).unwrap_err().contains("points"));
        assert!(matches!(parse(r#"{"type":"scroll","clicks":-2,"hold_keys":["ctrl"]}"#).unwrap(), ActionPrimitive::ScrollFine { delta: -240, .. }));
        assert!(matches!(parse(r#"{"type":"scroll","delta":-30}"#).unwrap(), ActionPrimitive::ScrollFine { delta: -30, .. }));
        assert!(matches!(parse(r#"{"type":"type_text","text":"வணக்கம்","method":"paste"}"#).unwrap(), ActionPrimitive::TypeWith { method: crate::input::TypeMethod::Paste, .. }));
        assert!(matches!(parse(r#"{"type":"type_text","text":"abc","chars_per_second":5}"#).unwrap(), ActionPrimitive::TypeWith { chars_per_second: Some(_), .. }));
        assert!(matches!(parse(r#"{"type":"key_press","key":"shift","hold_ms":1500}"#).unwrap(), ActionPrimitive::HoldKey { hold_ms: 1500, .. }));
        assert!(matches!(parse(r#"{"type":"key_down","key":"shift"}"#).unwrap(), ActionPrimitive::KeyDown { .. }));
        assert!(matches!(parse(r#"{"type":"zoom","x":10,"y":10,"width":200,"height":100}"#).unwrap(), ActionPrimitive::Zoom { width: 200, .. }));
        assert!(parse(r#"{"type":"zoom","x":10,"y":10}"#).unwrap_err().contains("width"));
        assert!(matches!(parse(r#"{"type":"observe"}"#).unwrap(), ActionPrimitive::Observe {}));
        assert!(matches!(parse(r#"{"type":"wait","seconds":2}"#).unwrap(), ActionPrimitive::WaitSettle { max_timeout_ms: 2000, .. }));
        assert!(matches!(parse(r#"{"type":"clipboard_write","text":"x"}"#).unwrap(), ActionPrimitive::ClipboardWrite { .. }));
        assert!(matches!(parse(r#"{"type":"snap_window","window":3,"side":"left"}"#).unwrap(), ActionPrimitive::WindowAction { command: crate::input::WindowCommand::Snap, window: Some(ref w), .. } if w == "w3"));
        assert!(matches!(parse(r#"{"type":"close_window"}"#).unwrap(), ActionPrimitive::WindowAction { command: crate::input::WindowCommand::Close, window: None, .. }));
        assert!(matches!(parse(r#"{"type":"layout_windows","windows":[1,"w2"]}"#).unwrap(), ActionPrimitive::LayoutWindows { ref windows, .. } if windows == &vec!["w1".to_owned(), "w2".to_owned()]));
        assert!(matches!(parse(r#"{"type":"swipe","x":1,"y":2,"to_x":100,"to_y":2}"#).unwrap(), ActionPrimitive::Touch { gesture: crate::input::TouchGesture::Swipe, .. }));
        assert!(parse(r#"{"type":"touch","gesture":"swipe","x":1,"y":2}"#).unwrap_err().contains("to_x"));
        assert!(matches!(parse(r#"{"type":"switch_desktop","direction":"right"}"#).unwrap(), ActionPrimitive::SwitchDesktop { .. }));
    }

    #[test]
    fn the_window_an_action_names_is_kept_for_the_registry_check() {
        let proposal = parse_proposal(&proposal_text(r#"{"type":"click","x":5,"y":6,"window":"w4"}"#, "")).unwrap();
        assert_eq!(proposal.target_window.as_deref(), Some("w4"));
        let command = parse_proposal(&proposal_text(r#"{"type":"minimize_window","window":"w4"}"#, "")).unwrap();
        assert_eq!(command.target_window, None, "a window command consumes its window");
    }

    #[test]
    fn the_schema_offers_every_new_action() {
        let schema = proposal_schema(5);
        let text = schema.to_string();
        for name in ACTION_TYPES {
            assert!(text.contains(&format!("\"{name}\"")), "{name} missing from the schema");
        }
    }

    #[test]
    fn ten_thousand_mangled_replies_never_panic_and_never_yield_an_unchecked_point() {
        // Deterministic pseudo-random mutations of valid replies: cut, duplicate, swap bytes,
        // insert junk. Every result must be a clean Ok or Err.
        let seeds = [
            proposal_text(r#"{"type":"click","x":640,"y":360}"#, r#","voice_command":"Clicking""#),
            proposal_text(r#"{"type":"draw_path","shape":{"kind":"spiral","cx":50,"cy":50,"r_end":40}}"#, ""),
            proposal_text(r#"{"type":"drag_and_drop","from_x":1,"from_y":2,"to_x":3,"to_y":4,"waypoints":[[1,2]]}"#, ""),
            proposal_text(r#"{"type":"type_text","text":"emoji 😀 தமிழ்","method":"keys"}"#, ""),
        ];
        let junk = ["{", "}", "\"", ",", ":", "null", "-1e309", "[[", "]]", "\\u0000", "NaN", "\"x\":\"9999999999999\""];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for i in 0..10_000 {
            let mut text = seeds[i % seeds.len()].clone().into_bytes();
            for _ in 0..(1 + next() % 4) {
                let position = (next() as usize) % text.len().max(1);
                match next() % 4 {
                    0 => text.truncate(position),
                    1 => {
                        let piece = junk[(next() as usize) % junk.len()].as_bytes();
                        text.splice(position..position, piece.iter().copied());
                    }
                    2 if text.len() > 2 => {
                        let len = text.len();
                        let other = (next() as usize) % len;
                        text.swap(position.min(len - 1), other);
                    }
                    _ => {
                        let copy = text[..position].to_vec();
                        text.extend(copy);
                    }
                }
            }
            let mangled = String::from_utf8_lossy(&text).into_owned();
            let _ = parse_proposal(&mangled); // must not panic
        }
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
    fn the_coordinate_dialects_of_common_vision_models_are_understood() {
        let take = |action: &str| {
            let raw = format!(r#"{{"previousOutcome":"not_applicable","reflection":"r","thought":"t","target":{{"description":"Save"}},"expectedOutcome":"e","impact":"routine","action":{action}}}"#);
            parse_proposal(&raw).map(|p| p.action)
        };
        let at = |a: Result<ActionPrimitive, String>| match a {
            Ok(ActionPrimitive::Click { x, y, .. }) => (x, y),
            other => panic!("not a click: {other:?}"),
        };
        assert_eq!(at(take(r#"{"type":"click","coordinate":[120,80]}"#)), (120, 80));
        assert_eq!(at(take(r#"{"action":"click","point":"(120, 80)"}"#)), (120, 80));
        assert_eq!(at(take(r#"{"type":"left_click","start_box":"<|box_start|>(120,80)<|box_end|>"}"#)), (120, 80));
        assert_eq!(at(take(r#"{"type":"Click","bbox":[100,60,140,100]}"#)), (120, 80));
        assert_eq!(at(take(r#"{"action_type":"click","position":{"x":"120","y":"80"}}"#)), (120, 80));
        assert!(matches!(take(r#"{"type":"hotkey","keys":"ctrl+s"}"#), Ok(ActionPrimitive::Hotkey { keys }) if keys == ["ctrl", "s"]));
        assert!(matches!(take(r#"{"type":"key_press","key":"ctrl+shift+n"}"#), Ok(ActionPrimitive::Hotkey { keys }) if keys.len() == 3));
        assert!(matches!(take(r#"{"type":"type_text","content":"hello"}"#), Ok(ActionPrimitive::TypeText { text, .. }) if text == "hello"));
        match take(r#"{"type":"drag_and_drop","start_coordinate":[10,20],"end_coordinate":[300,400]}"#) {
            Ok(ActionPrimitive::DragAndDrop { from_x, from_y, to_x, to_y, .. }) => assert_eq!((from_x, from_y, to_x, to_y), (10, 20, 300, 400)),
            other => panic!("{other:?}"),
        }
        // Whole-proposal dialects: a list of actions, and the action written at the top level.
        let list = r#"{"thought":"t","target":"Save button","actions":[{"type":"click","x":5,"y":6},{"type":"click","x":7,"y":8}]}"#;
        let p = parse_proposal(list).unwrap();
        assert!(matches!(p.action, ActionPrimitive::Click { x: 5, y: 6, .. }));
        assert_eq!(p.target.visible_text.as_deref(), Some("Save button"));
        let flat = r#"{"thought":"t","action":"click","coordinate":[9,10]}"#;
        assert!(matches!(parse_proposal(flat).unwrap().action, ActionPrimitive::Click { x: 9, y: 10, .. }));
        let top = r#"{"thought":"t","type":"type_text","text":"hi"}"#;
        assert!(matches!(parse_proposal(top).unwrap().action, ActionPrimitive::TypeText { .. }));
        // The schema's own fields win over a dialect field.
        assert_eq!(at(take(r#"{"type":"click","x":1,"y":2,"coordinate":[120,80]}"#)), (1, 2));
    }

    #[test]
    fn grounding_answers_parse_as_element_or_point() {
        let resolve = |id: u32| if id <= 3 { Ok((10 * id as i32, 5)) } else { Err("no such element".to_owned()) };
        assert_eq!(parse_grounding_with(r#"{"found":true,"element":2}"#, &resolve).unwrap(), (20, 5));
        assert_eq!(parse_grounding_with(r#"{"found":true,"x":101,"y":7}"#, &resolve).unwrap(), (101, 7));
        assert!(parse_grounding_with(r#"{"found":false}"#, &resolve).is_err());
        assert!(parse_grounding_with(r#"{"found":true}"#, &resolve).is_err());
        assert!(parse_grounding_with(r#"{"found":true,"element":9}"#, &resolve).is_err());
        assert_eq!(grounding_schema(5)["properties"]["element"]["maximum"], 5);
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
