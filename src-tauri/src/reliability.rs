use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::input::ActionPrimitive;
use crate::perception::{token_overlap_score, GroundingCandidate};

pub const MAX_TRAJECTORY_STEPS: usize = 8;
pub const MAX_FAILED_RETRIES: u8 = 2;
pub const MAX_NOTES: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    #[default]
    NotApplicable,
    Succeeded,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImpactClass {
    #[default]
    Routine,
    Destructive,
    Financial,
    Credential,
    Communication,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct TargetDescriptor {
    pub description: String,
    #[serde(default)]
    pub visible_text: Option<String>,
    #[serde(default)]
    pub control_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ActionProposal {
    pub step_id: u32,
    pub previous_outcome: StepOutcome,
    pub reflection: String,
    pub thought: String,
    pub target: TargetDescriptor,
    pub expected_outcome: String,
    pub action: ActionPrimitive,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub impact: ImpactClass,
    /// Short sentence spoken aloud before the action runs (optional; a phrase is made from the
    /// action when the model leaves it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_command: Option<String>,
    /// Window id (`w3`) the model says the action is for; checked against the window registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_window: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationVerdict {
    Accept,
    Correct,
    Replan,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifiedAction {
    pub verdict: VerificationVerdict,
    pub reason: String,
    #[serde(default)]
    pub final_action: Option<ActionPrimitive>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub grounding_evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    pub step: u32,
    pub thought: String,
    pub target: TargetDescriptor,
    pub expected_outcome: String,
    pub action: ActionPrimitive,
    pub outcome: StepOutcome,
    pub verification_reason: String,
    pub window_title: String,
    /// What the screen actually did after this action (measured, not claimed by the model).
    #[serde(default)]
    pub effect: String,
}

#[derive(Debug, Clone)]
pub struct TaskSession {
    pub run_id: String,
    pub task: String,
    pub history: VecDeque<StepRecord>,
    pub consecutive_failures: u8,
    /// Actions in a row whose measured effect was "nothing changed on screen".
    pub no_effect_streak: u8,
    /// Short facts the agent chose to remember (shown in every prompt).
    pub notes: Vec<String>,
    /// Instructions the user gave during the run, with the step after which each arrived.
    pub steers: Vec<(u32, String)>,
    /// What really happened on the computer, step by step (shown so nothing finished is repeated).
    pub ledger: Vec<LedgerEntry>,
    /// Interruptions and what changed during them ("paused 12 s; while paused: w4 opened").
    pub interruptions: Vec<String>,
}

/// One action that reached the computer.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub step: u32,
    pub what: String,
    pub status: LedgerStatus,
    /// Sending, submitting, saving, deleting, paying: never to be repeated without being asked.
    pub irreversible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerStatus {
    Done,
    /// Cut short by a pause, a stop or the user taking over.
    HalfDone,
}

const MAX_STEERS_SHOWN_IN_FULL: usize = 3;
const MAX_LEDGER_SHOWN: usize = 12;
const MAX_INTERRUPTIONS_SHOWN: usize = 3;

impl TaskSession {
    pub fn new(run_id: String, task: String) -> Self {
        Self {
            run_id,
            task,
            history: VecDeque::with_capacity(MAX_TRAJECTORY_STEPS),
            consecutive_failures: 0,
            no_effect_streak: 0,
            notes: Vec::new(),
            steers: Vec::new(),
            ledger: Vec::new(),
            interruptions: Vec::new(),
        }
    }

    pub fn add_steers(&mut self, after_step: u32, messages: Vec<String>) {
        self.steers.extend(messages.into_iter().map(|m| (after_step, m)));
    }

    pub fn record_action(&mut self, step: u32, what: String, status: LedgerStatus, irreversible: bool) {
        self.ledger.push(LedgerEntry { step, what, status, irreversible });
    }

    pub fn add_interruption(&mut self, text: String) {
        self.interruptions.push(text);
    }

    /// The task as it stands now: the original plus every change the user made during the run.
    /// The completion check judges this, not the original alone.
    pub fn effective_task(&self) -> String {
        if self.steers.is_empty() {
            return self.task.clone();
        }
        let changes: Vec<String> = self.steers.iter().map(|(_, m)| format!("- {m}")).collect();
        format!("{}\nChanges the user made during the run (the newest wins where they disagree):\n{}", self.task, changes.join("\n"))
    }

    /// Prompt section about the user's instructions, interruptions and what is already done.
    /// Kept short however long the run gets: the last few instructions in full, older ones
    /// counted; the last dozen ledger lines.
    pub fn control_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.steers.is_empty() {
            let older = self.steers.len().saturating_sub(MAX_STEERS_SHOWN_IN_FULL);
            let mut lines = vec!["USER INSTRUCTIONS GIVEN DURING THE RUN (follow them; they change the ORIGINAL TASK; the newest wins where they disagree):".to_owned()];
            if older > 0 {
                let summary: Vec<String> = self.steers[..older].iter().map(|(_, m)| m.chars().take(40).collect::<String>()).collect();
                lines.push(format!("- (earlier: {})", summary.join("; ")));
            }
            for (step, message) in &self.steers[older..] {
                lines.push(format!("- after step {step}: \"{message}\""));
            }
            parts.push(lines.join("\n"));
        }
        if !self.interruptions.is_empty() {
            let shown = &self.interruptions[self.interruptions.len().saturating_sub(MAX_INTERRUPTIONS_SHOWN)..];
            parts.push(format!("INTERRUPTIONS:\n{}", shown.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")));
        }
        if !self.ledger.is_empty() {
            let shown = &self.ledger[self.ledger.len().saturating_sub(MAX_LEDGER_SHOWN)..];
            let lines: Vec<String> = shown
                .iter()
                .map(|entry| {
                    let status = match entry.status {
                        LedgerStatus::Done => "done",
                        LedgerStatus::HalfDone => "CUT SHORT (half done: check the screen before continuing)",
                    };
                    let warning = if entry.irreversible && entry.status == LedgerStatus::Done { " - DO NOT REPEAT" } else { "" };
                    format!("- step {}: {} - {status}{warning}", entry.step, entry.what)
                })
                .collect();
            parts.push(format!("ALREADY DONE ON THE COMPUTER (do not redo finished work):\n{}", lines.join("\n")));
        }
        parts.join("\n\n")
    }

    pub fn push(&mut self, record: StepRecord) {
        if record.outcome == StepOutcome::Failed {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else if record.outcome == StepOutcome::Succeeded {
            self.consecutive_failures = 0;
        }
        if self.history.len() == MAX_TRAJECTORY_STEPS {
            self.history.pop_front();
        }
        self.history.push_back(record);
    }

    /// Records whether the latest action visibly did anything. Returns the new streak length.
    pub fn record_effect(&mut self, no_visible_effect: bool) -> u8 {
        self.no_effect_streak = if no_visible_effect { self.no_effect_streak.saturating_add(1) } else { 0 };
        self.no_effect_streak
    }

    pub fn add_note(&mut self, text: &str) {
        let note: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        if note.is_empty() || self.notes.contains(&note) {
            return;
        }
        if self.notes.len() >= MAX_NOTES {
            self.notes.remove(0);
        }
        self.notes.push(note);
    }

    pub fn notes_text(&self) -> String {
        if self.notes.is_empty() {
            "(none)".to_owned()
        } else {
            self.notes.iter().enumerate().map(|(i, n)| format!("{}. {n}", i + 1)).collect::<Vec<_>>().join("
")
        }
    }

    pub fn history_json(&self) -> String {
        serde_json::to_string(&self.history).unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn apply_previous_outcome(&mut self, outcome: StepOutcome, reflection: &str) {
        if let Some(last) = self.history.back_mut() {
            last.outcome = outcome.clone();
            if !reflection.trim().is_empty() {
                last.verification_reason = reflection.to_owned();
            }
        }
        match outcome {
            StepOutcome::Failed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1)
            }
            StepOutcome::Succeeded => self.consecutive_failures = 0,
            _ => {}
        }
    }

    pub fn repeated_action_loop(&self, action: &ActionPrimitive) -> bool {
        // Repeated `done` claims are limited by the completion check, not by this detector.
        if matches!(action, ActionPrimitive::Done { .. } | ActionPrimitive::Fail { .. }) {
            return false;
        }
        let current = serde_json::to_string(action).unwrap_or_default();
        self.history
            .iter()
            .rev()
            .take(3)
            .filter(|record| serde_json::to_string(&record.action).unwrap_or_default() == current)
            .count()
            >= 2
    }
}

/// Recognises a plain "open <app>" request (for example "open notepad" or "please launch the
/// Calculator app") so the first step can start the app directly instead of asking a model.
/// Anything with more going on ("open notepad and type hello", a file name, a web address)
/// returns `None` and goes through the normal model loop.
pub fn quick_open_target(task: &str) -> Option<String> {
    let lower_of = |word: &str| word.to_lowercase();
    let words: Vec<&str> = task.split_whitespace().collect();
    let mut index = 0;
    while index < words.len() && ["please", "kindly", "can", "could", "you"].contains(&lower_of(words[index]).as_str()) {
        index += 1;
    }
    let verb = lower_of(words.get(index)?);
    if !["open", "launch", "start", "run"].contains(&verb.as_str()) {
        return None;
    }
    let mut rest: Vec<&str> = words[index + 1..]
        .iter()
        .map(|word| word.trim_matches(|c: char| matches!(c, '.' | '!' | ',' | '"' | '\'')))
        .filter(|word| !word.is_empty())
        .collect();
    while rest.first().is_some_and(|w| ["the", "my", "a"].contains(&lower_of(w).as_str())) {
        rest.remove(0);
    }
    while rest.last().is_some_and(|w| ["please", "now", "app", "application", "program", "again"].contains(&lower_of(w).as_str())) {
        rest.pop();
    }
    if rest.is_empty() || rest.len() > 3 {
        return None;
    }
    const NOT_AN_APP_NAME: [&str; 12] =
        ["and", "then", "in", "on", "with", "to", "for", "folder", "website", "site", "page", "tab"];
    let name_like = rest.iter().all(|word| {
        word.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '+')
            && !NOT_AN_APP_NAME.contains(&lower_of(word).as_str())
    });
    // "file explorer" is an application; "file report" or "document" are not.
    let starts_with_file = lower_of(rest[0]) == "file";
    let is_file_explorer = rest.len() == 2 && starts_with_file && lower_of(rest[1]) == "explorer";
    if !name_like || ((starts_with_file || lower_of(rest[0]) == "document") && !is_file_explorer) {
        return None;
    }
    Some(rest.join(" "))
}

pub fn normalize_intent(task: &str) -> String {
    task.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

pub fn explicit_impact_authorized(task: &str, impact: &ImpactClass) -> bool {
    let task = task.to_lowercase();
    let has_any = |terms: &[&str]| terms.iter().any(|term| task.contains(term));
    match impact {
        ImpactClass::Routine => true,
        ImpactClass::Destructive => has_any(&[
            "delete", "remove", "empty recycle bin", "format", "drop table", "erase",
        ]),
        ImpactClass::Financial => has_any(&[
            "pay", "payment", "purchase", "buy", "transfer", "send money", "subscribe",
        ]),
        ImpactClass::Credential => has_any(&[
            "change password", "reset password", "credential", "passcode", "security key",
        ]),
        ImpactClass::Communication => {
            (task.contains("send") && has_any(&["email", "message", "reply"]))
                || has_any(&["reply to", "post", "publish", "submit form"])
        }
    }
}

pub fn effective_impact(proposal: &ActionProposal) -> ImpactClass {
    if proposal.impact != ImpactClass::Routine {
        return proposal.impact.clone();
    }
    let mut context = format!(
        "{} {} {} {}",
        proposal.thought,
        proposal.target.description,
        proposal.target.visible_text.as_deref().unwrap_or_default(),
        proposal.expected_outcome
    )
    .to_lowercase();
    if let ActionPrimitive::TypeText { text, .. } = &proposal.action {
        context.push(' ');
        context.push_str(&text.to_lowercase());
    }
    let has = |terms: &[&str]| terms.iter().any(|term| context.contains(term));
    if has(&["delete", "remove", "erase", "format drive", "drop table", "empty recycle"]) {
        ImpactClass::Destructive
    } else if has(&["pay", "purchase", "buy now", "transfer money", "subscribe"]) {
        ImpactClass::Financial
    } else if has(&["password", "passcode", "credential", "security key"]) {
        ImpactClass::Credential
    } else if has(&["send email", "send message", "reply", "publish", "post now", "submit form"]) {
        ImpactClass::Communication
    } else {
        ImpactClass::Routine
    }
}

pub fn best_grounding_candidate<'a>(
    target: &TargetDescriptor,
    candidates: &'a [GroundingCandidate],
) -> Option<(&'a GroundingCandidate, f32)> {
    let query = target
        .visible_text
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&target.description);
    let mut scored = candidates
        .iter()
        .map(|candidate| (candidate, token_overlap_score(query, &candidate.label)))
        .filter(|(_, score)| *score >= 0.6)
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    let (best, score) = *scored.first()?;
    let runner_up = scored.get(1).map(|value| value.1).unwrap_or(0.0);
    if score - runner_up < 0.15 && runner_up >= 0.6 {
        return None;
    }
    Some((best, score))
}

pub fn apply_candidate(action: ActionPrimitive, candidate: &GroundingCandidate) -> ActionPrimitive {
    let (x, y) = candidate.center();
    match action {
        ActionPrimitive::Click {
            button,
            clicks,
            hold_keys,
            ..
        } => ActionPrimitive::Click {
            x,
            y,
            button,
            clicks,
            hold_keys,
        },
        ActionPrimitive::DoubleClick { button, .. } => {
            ActionPrimitive::DoubleClick { x, y, button }
        }
        ActionPrimitive::MouseMove { .. } => ActionPrimitive::MouseMove { x, y },
        ActionPrimitive::Scroll {
            clicks,
            horizontal,
            ..
        } => ActionPrimitive::Scroll {
            clicks,
            x: Some(x),
            y: Some(y),
            horizontal,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consequential_actions_require_explicit_task_language() {
        assert!(explicit_impact_authorized(
            "Send an email to Sam",
            &ImpactClass::Communication
        ));
        assert!(!explicit_impact_authorized(
            "Open my inbox",
            &ImpactClass::Communication
        ));
    }

    #[test]
    fn no_effect_streak_counts_and_resets() {
        let mut session = TaskSession::new("run".into(), "task".into());
        assert_eq!(session.record_effect(true), 1);
        assert_eq!(session.record_effect(true), 2);
        assert_eq!(session.record_effect(false), 0);
        assert_eq!(session.record_effect(true), 1);
    }

    #[test]
    fn notes_are_deduplicated_trimmed_and_capped() {
        let mut session = TaskSession::new("run".into(), "task".into());
        assert_eq!(session.notes_text(), "(none)");
        session.add_note("  order   #4411 
 shipped ");
        session.add_note("order #4411 shipped");
        assert_eq!(session.notes, ["order #4411 shipped"]);
        for i in 0..20 {
            session.add_note(&format!("fact {i}"));
        }
        assert_eq!(session.notes.len(), MAX_NOTES);
        assert!(session.notes_text().contains("fact 19") && !session.notes_text().contains("fact 0
"));
    }

    #[test]
    fn plain_open_requests_are_recognised_and_complex_ones_are_not() {
        for (task, app) in [
            ("open notepad", "notepad"),
            ("Open Notepad now", "Notepad"),
            ("please launch the Calculator app.", "Calculator"),
            ("can you start Google Chrome", "Google Chrome"),
            ("open file explorer", "file explorer"),
            ("run paint", "paint"),
        ] {
            assert_eq!(quick_open_target(task).as_deref(), Some(app), "{task}");
        }
        for task in [
            "open notepad and type hello",
            "open notepad then save it",
            "open report.docx",
            "open https://example.com",
            "open the file budget",
            "open a new tab",
            "open the settings page in chrome",
            "type hello in notepad",
            "open",
            "open one two three four",
        ] {
            assert_eq!(quick_open_target(task), None, "{task}");
        }
    }

    #[test]
    fn repeated_done_claims_are_not_a_click_loop() {
        let mut session = TaskSession::new("run".into(), "task".into());
        let done = ActionPrimitive::Done { summary: "x".into() };
        for step in 0..4 {
            session.push(StepRecord {
                step,
                thought: String::new(),
                target: TargetDescriptor::default(),
                expected_outcome: String::new(),
                action: done.clone(),
                outcome: StepOutcome::Failed,
                verification_reason: String::new(),
                window_title: String::new(),
                effect: String::new(),
            });
        }
        assert!(!session.repeated_action_loop(&done));
        let click = ActionPrimitive::Hotkey { keys: vec!["a".into()] };
        for step in 0..3 {
            session.push(StepRecord { step, action: click.clone(), ..session.history.back().unwrap().clone() });
        }
        assert!(session.repeated_action_loop(&click));
    }

    #[test]
    fn history_is_bounded() {
        let mut session = TaskSession::new("run".into(), "task".into());
        for step in 0..12 {
            session.push(StepRecord {
                step,
                thought: String::new(),
                target: TargetDescriptor::default(),
                expected_outcome: String::new(),
                action: ActionPrimitive::Done { summary: String::new() },
                outcome: StepOutcome::Succeeded,
                verification_reason: String::new(),
                window_title: String::new(),
                effect: String::new(),
            });
        }
        assert_eq!(session.history.len(), MAX_TRAJECTORY_STEPS);
        assert_eq!(session.history.front().unwrap().step, 4);
    }

    #[test]
    fn ten_steers_and_a_long_run_keep_the_prompt_section_short_and_clear() {
        let mut session = TaskSession::new("r".into(), "write an email".into());
        for i in 1..=10 {
            session.add_steers(i, vec![format!("instruction number {i} with some words")]);
        }
        for step in 1..=30 {
            session.record_action(step, format!("action {step}"), LedgerStatus::Done, step == 7);
        }
        session.record_action(31, "drag the file".into(), LedgerStatus::HalfDone, false);
        session.add_interruption("the run was paused for 12 s".into());
        let text = session.control_text();
        // The newest three instructions in full, the older ones summarised in one line.
        assert!(text.contains("after step 10: \"instruction number 10"));
        assert!(text.contains("after step 8:"));
        assert!(!text.contains("after step 7:"), "older instructions are not listed one by one");
        assert!(text.contains("(earlier: instruction number 1"));
        // Only the latest dozen ledger lines, with the cut-short one flagged.
        assert!(!text.contains("step 7: action 7"), "old ledger lines are trimmed");
        assert!(text.contains("step 31: drag the file - CUT SHORT"));
        assert!(text.lines().count() < 30, "the section stays short: {} lines", text.lines().count());
        assert!(session.effective_task().contains("instruction number 10"));
    }

    #[test]
    fn finished_submits_are_marked_do_not_repeat() {
        let mut session = TaskSession::new("r".into(), "send it".into());
        session.record_action(3, "press Enter".into(), LedgerStatus::Done, true);
        session.record_action(4, "type \"hi\"".into(), LedgerStatus::Done, false);
        let text = session.control_text();
        assert!(text.contains("step 3: press Enter - done - DO NOT REPEAT"));
        assert!(text.contains("step 4: type \"hi\" - done\n") || text.ends_with("step 4: type \"hi\" - done"));
        assert!(TaskSession::new("r".into(), "t".into()).control_text().is_empty(), "nothing to say on a fresh run");
    }

    #[test]
    fn local_classifier_catches_model_underclassification() {
        let proposal = ActionProposal {
            step_id: 1,
            previous_outcome: StepOutcome::NotApplicable,
            reflection: String::new(),
            thought: "Click Delete permanently".into(),
            target: TargetDescriptor {
                description: "Delete button".into(),
                ..Default::default()
            },
            expected_outcome: "File is removed".into(),
            action: ActionPrimitive::Click {
                x: 1,
                y: 1,
                button: Default::default(),
                clicks: 1,
                hold_keys: Vec::new(),
            },
            confidence: 1.0,
            impact: ImpactClass::Routine,
            voice_command: None,
            target_window: None,
        };
        assert_eq!(effective_impact(&proposal), ImpactClass::Destructive);
    }
}
