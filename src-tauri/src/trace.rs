//! Per-run debug trace written to disk so any run can be diagnosed afterwards.
//!
//! Layout: `<data_root>/runs/<run_id>/run.jsonl` plus `step_NNN/` folders holding the
//! screenshots, raw model replies, chosen action and timings of each step.
//!
//! Tracing is strictly best-effort: a write failure is logged to stderr and never affects the
//! run. API keys are never passed in here, only task data, model output and images.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

/// Hides anything in `text` that looks like a secret before it is written to a trace: values after
/// words like password, api key, token, secret or Bearer, and long mixed letter-and-digit tokens
/// (API keys, JWTs). Traces are for diagnosing runs, never for keeping credentials.
pub fn mask_secrets(text: &str) -> String {
    const WORDS: [&str; 8] = ["password", "passwd", "secret", "api_key", "apikey", "api-key", "token", "bearer"];
    let is_value = |c: char| c.is_ascii_alphanumeric() || "_-.~+/=!@#$%^&*".contains(c);
    let lower = text.to_ascii_lowercase();
    let mut hidden = vec![false; text.len()];
    for word in WORDS {
        let mut from = 0;
        while let Some(found) = lower[from..].find(word) {
            let end = from + found + word.len();
            let rest = &text[end..];
            // Skip separators between the word and its value: quotes, colon, equals, spaces.
            let skip = rest.len() - rest.trim_start_matches(|c: char| matches!(c, '"' | '\'' | ':' | '=' | ' ' | '\\')).len();
            let value_start = end + skip;
            // A bare word followed directly by more letters ("tokens", "passwords") is not a key.
            if skip > 0 || word == "bearer" {
                let len = text[value_start..].find(|c: char| !is_value(c)).unwrap_or(text.len() - value_start);
                if len >= 3 {
                    for flag in &mut hidden[value_start..value_start + len] {
                        *flag = true;
                    }
                }
            }
            from = end;
        }
    }
    // Long tokens that mix letters and digits, or that look like well-known key formats.
    let mut index = 0;
    for piece in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')) {
        let start = index;
        index += piece.len() + 1;
        let has_digit = piece.chars().any(|c| c.is_ascii_digit());
        let has_alpha = piece.chars().any(|c| c.is_ascii_alphabetic());
        let known = piece.starts_with("sk-") || piece.starts_with("ghp_") || piece.starts_with("xox") || piece.starts_with("eyJ") || piece.starts_with("AKIA");
        if piece.len() >= 24 && has_digit && has_alpha || (known && piece.len() >= 12) {
            if let Some(range) = hidden.get_mut(start..start + piece.len()) {
                range.iter_mut().for_each(|f| *f = true);
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut in_hidden = false;
    for (i, ch) in text.char_indices() {
        if hidden[i] {
            if !in_hidden {
                out.push_str("[hidden]");
                in_hidden = true;
            }
        } else {
            in_hidden = false;
            out.push(ch);
        }
    }
    out
}

/// How many run folders to keep; older ones are deleted when a new run starts.
const KEEP_RUNS: usize = 30;

#[derive(Debug, Clone)]
pub struct TraceWriter {
    dir: Option<PathBuf>,
}

/// Handle for one step's folder. Cheap to clone and pass around.
#[derive(Debug, Clone)]
pub struct StepTrace {
    dir: Option<PathBuf>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn warn(context: &str, error: &dyn std::fmt::Display) {
    eprintln!("[aura-trace] {context}: {error}");
}

impl TraceWriter {
    /// A writer that records nothing (used when the folder cannot be created).
    pub fn disabled() -> Self {
        Self { dir: None }
    }

    pub fn start(data_root: &Path, run_id: &str, task: &str, provider_summary: Value) -> Self {
        let runs = data_root.join("runs");
        if let Err(error) = fs::create_dir_all(&runs) {
            warn("create runs dir", &error);
            return Self::disabled();
        }
        prune_old_runs(&runs, KEEP_RUNS);
        let dir = runs.join(run_id);
        if let Err(error) = fs::create_dir_all(&dir) {
            warn("create run dir", &error);
            return Self::disabled();
        }
        let writer = Self { dir: Some(dir) };
        writer.event(0, "run_start", json!({ "task": task, "provider": provider_summary }));
        writer
    }

    pub fn path(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Appends one line to `run.jsonl`.
    pub fn event(&self, step: u32, kind: &str, data: Value) {
        let Some(dir) = &self.dir else { return };
        let line = mask_secrets(&json!({ "t_ms": now_ms() as u64, "step": step, "kind": kind, "data": data }).to_string());
        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("run.jsonl"))
            .and_then(|mut file| writeln!(file, "{line}"));
        if let Err(error) = result {
            warn("write run.jsonl", &error);
        }
    }

    pub fn step(&self, step: u32) -> StepTrace {
        let Some(dir) = &self.dir else { return StepTrace { dir: None } };
        let step_dir = dir.join(format!("step_{step:03}"));
        match fs::create_dir_all(&step_dir) {
            Ok(()) => StepTrace { dir: Some(step_dir) },
            Err(error) => {
                warn("create step dir", &error);
                StepTrace { dir: None }
            }
        }
    }
}

impl StepTrace {
    pub fn bytes(&self, name: &str, data: &[u8]) {
        let Some(dir) = &self.dir else { return };
        if let Err(error) = fs::write(dir.join(name), data) {
            warn(name, &error);
        }
    }

    pub fn text(&self, name: &str, data: &str) {
        self.bytes(name, mask_secrets(data).as_bytes());
    }

    pub fn json<T: Serialize>(&self, name: &str, value: &T) {
        match serde_json::to_vec_pretty(value) {
            Ok(bytes) => self.bytes(name, mask_secrets(&String::from_utf8_lossy(&bytes)).as_bytes()),
            Err(error) => warn(name, &error),
        }
    }
}

fn prune_old_runs(runs: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(runs) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    // Run ids embed a millisecond timestamp, so lexical order is chronological.
    dirs.sort();
    // Keep room for the run about to be created.
    let excess = (dirs.len() + 1).saturating_sub(keep);
    for old in dirs.into_iter().take(excess) {
        if let Err(error) = fs::remove_dir_all(&old) {
            warn("prune old run", &error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("aura-trace-test-{name}-{}", now_ms()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn writes_events_and_step_files() {
        let root = temp_root("write");
        let trace = TraceWriter::start(&root, "run_1", "do a thing", json!({"model": "m"}));
        trace.event(1, "observe", json!({"ms": 12}));
        let step = trace.step(1);
        step.text("proposal_raw.txt", "{\"a\":1}");
        step.json("action.json", &json!({"type": "click"}));
        step.bytes("obs.png", &[1, 2, 3]);

        let run_dir = root.join("runs").join("run_1");
        let lines = fs::read_to_string(run_dir.join("run.jsonl")).unwrap();
        assert_eq!(lines.lines().count(), 2);
        assert!(lines.contains("\"kind\":\"run_start\""));
        assert_eq!(fs::read(run_dir.join("step_001").join("obs.png")).unwrap(), vec![1, 2, 3]);
        assert!(run_dir.join("step_001").join("action.json").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn secrets_are_hidden_but_ordinary_text_is_not() {
        let masked = mask_secrets(r#"{"api_key":"sk-abcdef1234567890abcdef","password": "hunter2!","note":"Authorization: Bearer abc123def456ghi789"}"#);
        for secret in ["sk-abcdef1234567890abcdef", "hunter2", "abc123def456ghi789"] {
            assert!(!masked.contains(secret), "{masked}");
        }
        assert!(masked.contains("[hidden]"));
        let plain = "click the Save button in the Notepad window, tokens and passwords are discussed in chapter 4";
        assert_eq!(mask_secrets(plain), plain);
        let long_token = mask_secrets("key=AbCdEfGhIjKlMnOpQrStUvWx12345 end");
        assert!(!long_token.contains("AbCdEf") && long_token.ends_with(" end"), "{long_token}");
        assert_eq!(mask_secrets("a window title with ünïcode 日本語"), "a window title with ünïcode 日本語");
    }

    #[test]
    fn traces_are_masked_on_disk() {
        let root = temp_root("mask");
        let trace = TraceWriter::start(&root, "run_m", "t", json!({}));
        trace.event(1, "x", json!({"password": "topsecret99"}));
        trace.step(1).text("raw.txt", "Bearer zzzzzzzzzzzz1111");
        let dir = root.join("runs").join("run_m");
        assert!(!fs::read_to_string(dir.join("run.jsonl")).unwrap().contains("topsecret99"));
        assert!(!fs::read_to_string(dir.join("step_001").join("raw.txt")).unwrap().contains("zzzzzzzzzzzz1111"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_writer_is_a_silent_no_op() {
        let trace = TraceWriter::disabled();
        trace.event(1, "x", json!({}));
        trace.step(1).text("a.txt", "b");
        assert!(trace.path().is_none());
    }

    #[test]
    fn prunes_oldest_runs_keeping_the_limit() {
        let root = temp_root("prune");
        let runs = root.join("runs");
        fs::create_dir_all(&runs).unwrap();
        for index in 0..5 {
            fs::create_dir_all(runs.join(format!("run_{index:04}"))).unwrap();
        }
        prune_old_runs(&runs, 3);
        let mut left: Vec<_> = fs::read_dir(&runs).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["run_0003", "run_0004"]);
        let _ = fs::remove_dir_all(&root);
    }
}
