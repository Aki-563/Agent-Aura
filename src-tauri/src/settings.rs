//! Harness settings (not model or provider settings): voice, input yielding, grounding mode,
//! per-app profiles path, scheduler options. Stored as JSON in
//! `%LOCALAPPDATA%\AgentAura\harness.json`; every field has a default, so a missing or partial
//! file is fine. Environment variables override a few fields for tests (`AURA_MUTE`, `AURA_VOICE`).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceSettings {
    /// Speak `voice_command` before each action.
    pub enabled: bool,
    /// 0..100
    pub volume: u16,
    /// -10 (slow) .. 10 (fast), SAPI's scale.
    pub rate: i32,
    /// Part of the installed voice's name ("Zira", "David"); empty = the system default voice.
    pub voice: String,
    /// Wait for the sentence to finish before acting (default: speak and act at the same time).
    pub speak_then_act: bool,
    /// A sentence that has played this long may be cut off by a newer one.
    pub min_play_ms: u64,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { enabled: true, volume: 80, rate: 1, voice: String::new(), speak_then_act: false, min_play_ms: 700 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroundingMode {
    /// One screenshot, one model call returns the action and its coordinates.
    #[default]
    A,
    /// The proposal describes the target; a second call on the same screenshot points at it.
    B,
    /// One call, then harness-side refinement (zoom crop, snap to element, hover check).
    C,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HarnessSettings {
    pub voice: VoiceSettings,
    /// Pause the run (cutting the current action) when the user touches the mouse or keyboard.
    pub yield_to_user: bool,
    pub grounding_mode: GroundingMode,
    /// Optional separate grounding model for mode B (empty = the main model).
    pub grounding_model: String,
    pub grounding_url: String,
    /// How the model writes raw x/y: "pixels" (image pixels), "norm1000" (0..1000) or "norm1"
    /// (0..1). Measured per model with `aura-cli calibrate`; "pixels" by default.
    pub coordinate_scale: String,
    /// Longest edge of the screenshot sent to the model.
    pub max_image_edge: u32,
    /// Hard limits for one run.
    pub max_steps: u32,
    pub max_minutes: u32,
    pub max_model_calls: u32,
    /// Skip the model call when nothing on screen changed (waits and scheduled runs only).
    pub skip_unchanged: bool,
    /// Local control port for `aura-cli` and tests (0 = off).
    pub control_port: u16,
}

impl Default for HarnessSettings {
    fn default() -> Self {
        Self {
            voice: VoiceSettings::default(),
            yield_to_user: true,
            grounding_mode: GroundingMode::A,
            grounding_model: String::new(),
            grounding_url: String::new(),
            coordinate_scale: "pixels".into(),
            max_image_edge: 1280,
            max_steps: 40,
            max_minutes: 15,
            max_model_calls: 120,
            skip_unchanged: true,
            control_port: 0,
        }
    }
}

static CURRENT: Mutex<Option<HarnessSettings>> = Mutex::new(None);

pub fn path() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("AgentAura").join("harness.json")
}

/// Current settings (loaded once from disk, then cached).
pub fn get() -> HarnessSettings {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if current.is_none() {
        let loaded = std::fs::read_to_string(path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        *current = Some(loaded);
    }
    let mut settings = current.clone().unwrap_or_default();
    if cfg!(test) || std::env::var_os("AURA_MUTE").is_some() || std::env::var("AURA_VOICE").map(|v| v == "0").unwrap_or(false) {
        settings.voice.enabled = false;
    }
    settings
}

/// Replaces the settings for this process only; nothing is written to disk (tests switch the
/// grounding mode this way without touching the user's harness.json).
pub fn set_in_memory(settings: HarnessSettings) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings);
}

/// Replaces the settings and writes them to disk.
pub fn set(settings: HarnessSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    if let Some(parent) = path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path(), text).map_err(|e| format!("could not save {}: {e}", path().display()))?;
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_file_keeps_defaults_for_everything_else() {
        let parsed: HarnessSettings = serde_json::from_str(r#"{"voice":{"rate":4},"grounding_mode":"c"}"#).unwrap();
        assert_eq!(parsed.voice.rate, 4);
        assert_eq!(parsed.voice.volume, 80);
        assert_eq!(parsed.grounding_mode, GroundingMode::C);
        assert_eq!(parsed.max_steps, 40);
        assert!(!get().voice.enabled, "voice is always muted under cargo test");
    }
}
