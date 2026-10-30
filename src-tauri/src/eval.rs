//! Automatic task suite: runs small, reversible Windows tasks through the real agent loop and
//! checks the outcome without trusting the model's own claim of success.
//!
//! Safety rules: tasks only create files under `%TEMP%\AuraEval`, only close windows that did
//! not exist before the task started, and never touch the network or the user's documents.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::agent::{execute_run, model_calls_so_far, visible_text_snapshot, AgentRuntime, Host};
use crate::desktop::target as desktop_target;
use crate::input::{ActionPrimitive, InputActuator};

/// How an outcome is verified, independently of the model.
#[derive(Debug, Clone)]
pub enum Check {
    /// The window in front has this text in its title (case-insensitive).
    ForegroundTitle(&'static str),
    /// Some text visible on screen (UI Automation names or OCR) contains this (case-insensitive).
    ScreenText(&'static str),
    /// A file in the sandbox folder contains this text.
    FileContains { name: &'static str, text: &'static str },
    /// A test page (see `eval_fixture`) reported this event, e.g. `click:beta`.
    Event(&'static str),
}

#[derive(Debug, Clone)]
pub struct EvalTask {
    pub id: &'static str,
    pub prompt: &'static str,
    pub max_steps: u32,
    pub check: Check,
    /// Select all and delete in the task's window before closing (so no test text lingers in
    /// apps that restore their last session).
    pub clear_text_on_cleanup: bool,
}

pub fn sandbox_dir() -> PathBuf {
    std::env::temp_dir().join("AuraEval")
}

pub fn suite(name: &str) -> Vec<EvalTask> {
    let basic = vec![
        EvalTask { id: "open-notepad", prompt: "open notepad", max_steps: 4, check: Check::ForegroundTitle("notepad"), clear_text_on_cleanup: false },
        EvalTask { id: "open-calculator", prompt: "open calculator", max_steps: 4, check: Check::ForegroundTitle("calculator"), clear_text_on_cleanup: false },
        EvalTask { id: "open-paint", prompt: "open paint", max_steps: 4, check: Check::ForegroundTitle("paint"), clear_text_on_cleanup: false },
        EvalTask { id: "open-file-explorer", prompt: "open file explorer", max_steps: 4, check: Check::ForegroundTitle("file explorer"), clear_text_on_cleanup: false },
        EvalTask { id: "open-settings", prompt: "open settings", max_steps: 4, check: Check::ForegroundTitle("settings"), clear_text_on_cleanup: false },
        EvalTask { id: "notepad-type", prompt: "open notepad and type the sentence hello from aura", max_steps: 8, check: Check::ScreenText("hello from aura"), clear_text_on_cleanup: true },
        EvalTask { id: "calculator-sum", prompt: "open calculator and work out 12 plus 30", max_steps: 12, check: Check::ScreenText("display is 42"), clear_text_on_cleanup: false },
    ];
    match name {
        "basic" => basic,
        "web" => web_tasks(),
        "intermediate" => vec![EvalTask {
            id: "notepad-save-file",
            prompt: "open notepad, type hello from aura, and save the file as aura_note.txt in the folder AuraEval inside the Temp folder",
            max_steps: 20,
            check: Check::FileContains { name: "aura_note.txt", text: "hello from aura" },
            clear_text_on_cleanup: true,
        }],
        "all" => {
            let mut all = basic;
            all.extend(suite("web"));
            all.extend(suite("intermediate"));
            all
        }
        _ => Vec::new(),
    }
}

fn web_task(id: &'static str, prompt: &'static str, max_steps: u32, event: &'static str) -> EvalTask {
    EvalTask { id, prompt, max_steps, check: Check::Event(event), clear_text_on_cleanup: false }
}

/// Tasks on local test pages: every input type, checked by the page itself.
fn web_tasks() -> Vec<EvalTask> {
    vec![
        web_task("web-click", "open http://127.0.0.1:47831/click in the browser and click the Beta button", 8, "click:beta"),
        web_task("web-form", "open http://127.0.0.1:47831/form in the browser, type the name Jane Doe, choose the color Blue, tick the agree checkbox and press Submit form", 16, "form:name=Jane Doe;color=blue;agree=true"),
        web_task("web-scroll", "open http://127.0.0.1:47831/scroll in the browser, scroll down to the bottom of the page and click the Finish button", 10, "click:finish"),
        web_task("web-doubleclick", "open http://127.0.0.1:47831/mouse in the browser and double-click the box that says Double-click me", 8, "dblclick:left"),
        web_task("web-rightclick", "open http://127.0.0.1:47831/mouse in the browser and right-click the box that says Right-click me", 8, "rightclick:right"),
        web_task("web-drag", "open http://127.0.0.1:47831/drag in the browser and drag the orange square into the dashed box", 8, "drop:ok"),
        web_task("web-cookies", "open http://127.0.0.1:47831/cookies in the browser, accept the cookie notice and then press Continue", 10, "click:continue"),
        web_task("web-link", "open http://127.0.0.1:47831/links in the browser and open the Pricing page", 8, "visit:pricing"),
    ]
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub id: String,
    pub passed: bool,
    pub run_completed: bool,
    pub steps_executed: u32,
    pub model_calls: u64,
    pub seconds: f64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteReport {
    pub model: String,
    pub suite: String,
    pub passed: usize,
    pub total: usize,
    pub results: Vec<TaskResult>,
}

impl SuiteReport {
    pub fn table(&self) -> String {
        let mut text = format!("{:<22} {:<6} {:>5} {:>7} {:>8}  {}\n", "task", "result", "steps", "calls", "seconds", "note");
        for result in &self.results {
            text.push_str(&format!(
                "{:<22} {:<6} {:>5} {:>7} {:>8.1}  {}\n",
                result.id,
                if result.passed { "PASS" } else { "FAIL" },
                result.steps_executed,
                result.model_calls,
                result.seconds,
                result.message.chars().take(70).collect::<String>()
            ));
        }
        text.push_str(&format!("\n{} / {} passed ({} model: {})", self.passed, self.total, self.suite, self.model));
        text
    }
}

/// Evaluates `check` against the current screen / sandbox. Retries briefly so slow UI settles.
pub fn evaluate(check: &Check) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let verdict = evaluate_once(check);
        if verdict.is_ok() || Instant::now() >= deadline {
            return verdict;
        }
        std::thread::sleep(Duration::from_millis(700));
    }
}

fn evaluate_once(check: &Check) -> Result<(), String> {
    match check {
        Check::ForegroundTitle(needle) => {
            let title = desktop_target::foreground_title();
            if title.to_lowercase().contains(needle) {
                Ok(())
            } else {
                Err(format!("the window in front is '{title}', not one containing '{needle}'"))
            }
        }
        Check::ScreenText(needle) => {
            let labels = visible_text_snapshot()?;
            if labels.iter().any(|label| label.to_lowercase().contains(needle)) {
                Ok(())
            } else {
                Err(format!("'{needle}' is not visible on screen"))
            }
        }
        Check::Event(wanted) => {
            if crate::eval_fixture::saw(wanted) {
                Ok(())
            } else {
                Err(format!("the test page never reported '{wanted}' (it saw: {})", crate::eval_fixture::events().join(", ")))
            }
        }
        Check::FileContains { name, text } => {
            let path = sandbox_dir().join(name);
            match std::fs::read_to_string(&path) {
                Ok(content) if content.to_lowercase().contains(text) => Ok(()),
                Ok(_) => Err(format!("{} exists but does not contain '{text}'", path.display())),
                Err(_) => Err(format!("{} does not exist", path.display())),
            }
        }
    }
}

/// Closes windows that appeared during the task (never anything that was open before it).
fn cleanup(before: &[isize], task: &EvalTask) {
    // A test page opened as a tab in an already-open browser is not a new window: close the tab.
    if desktop_target::foreground_title().contains("Aura Fixture") {
        let _ = InputActuator::execute_action(&ActionPrimitive::Hotkey { keys: vec!["ctrl".into(), "w".into()] });
        std::thread::sleep(Duration::from_millis(500));
    }
    let new_windows: Vec<isize> = desktop_target::list_candidate_windows()
        .into_iter()
        .filter(|window| !before.contains(window))
        .collect();
    if new_windows.is_empty() {
        return;
    }
    if task.clear_text_on_cleanup {
        if let Some(front) = desktop_target::effective_foreground() {
            // Select-all + Delete only ever inside Notepad: in File Explorer it would delete files.
            let is_notepad = desktop_target::foreground_title().to_lowercase().ends_with("notepad");
            if new_windows.contains(&(front as isize)) && is_notepad {
                let _ = InputActuator::execute_action(&ActionPrimitive::Hotkey { keys: vec!["ctrl".into(), "a".into()] });
                let _ = InputActuator::execute_action(&ActionPrimitive::KeyPress { key: "delete".into() });
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    for window in new_windows {
        desktop_target::close_window(window);
    }
    std::thread::sleep(Duration::from_millis(800));
}

/// Runs a suite against the provider already configured on `runtime`.
pub async fn run_suite(runtime: &AgentRuntime, host: &Host, name: &str, only: Option<&str>, model: &str) -> SuiteReport {
    let tasks: Vec<EvalTask> = suite(name).into_iter().filter(|task| only.is_none_or(|id| task.id == id)).collect();
    let _ = std::fs::create_dir_all(sandbox_dir());
    if tasks.iter().any(|task| matches!(task.check, Check::Event(_))) && !crate::eval_fixture::start() {
        eprintln!("warning: the test page server could not start on port {}", crate::eval_fixture::PORT);
    }
    let mut results = Vec::new();
    for task in &tasks {
        crate::eval_fixture::clear();
        let _ = std::fs::remove_file(sandbox_dir().join("aura_note.txt"));
        let before = desktop_target::list_candidate_windows();
        let calls_before = model_calls_so_far();
        let started = Instant::now();
        let run = execute_run(runtime, host, task.prompt.to_owned(), task.max_steps).await;
        let seconds = started.elapsed().as_secs_f64();
        let model_calls = model_calls_so_far() - calls_before;
        let (run_completed, steps_executed, run_message) = match &run {
            Ok(response) => (response.completed, response.steps_executed, response.message.clone()),
            Err(error) => (false, 0, error.clone()),
        };
        // The check is independent of what the model claimed.
        let check = tokio::task::spawn_blocking({
            let check = task.check.clone();
            move || evaluate(&check)
        })
        .await
        .unwrap_or_else(|error| Err(format!("check crashed: {error}")));
        let passed = check.is_ok();
        let message = match check {
            Ok(()) => run_message,
            Err(reason) => format!("{reason} (agent said: {run_message})"),
        };
        let cleanup_task = task.clone();
        let cleanup_before = before.clone();
        let _ = tokio::task::spawn_blocking(move || cleanup(&cleanup_before, &cleanup_task)).await;
        results.push(TaskResult { id: task.id.to_owned(), passed, run_completed, steps_executed, model_calls, seconds, message });
    }
    let passed = results.iter().filter(|result| result.passed).count();
    SuiteReport { model: model.to_owned(), suite: name.to_owned(), passed, total: results.len(), results }
}

/// Writes the report as JSON under `<data_root>/eval/` and returns the path.
pub fn save_report(runtime: &AgentRuntime, report: &SuiteReport) -> Option<PathBuf> {
    let dir = runtime.data_root().join("eval");
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let safe_model: String = report.model.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let path = dir.join(format!("{stamp}-{}-{safe_model}.json", report.suite));
    std::fs::write(&path, serde_json::to_vec_pretty(report).ok()?).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suites_have_unique_ids_and_sane_limits() {
        for name in ["basic", "web", "intermediate", "all"] {
            let tasks = suite(name);
            assert!(!tasks.is_empty(), "{name}");
            let mut ids: Vec<_> = tasks.iter().map(|t| t.id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), tasks.len(), "duplicate ids in {name}");
            assert!(tasks.iter().all(|t| (1..=40).contains(&t.max_steps) && !t.prompt.is_empty()));
        }
        assert!(suite("nope").is_empty());
        assert_eq!(suite("all").len(), suite("basic").len() + suite("web").len() + suite("intermediate").len());
    }

    #[test]
    fn file_check_reads_only_the_sandbox() {
        let dir = sandbox_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let name = "eval_selftest.txt";
        std::fs::write(dir.join(name), "Hello From Aura").unwrap();
        assert!(evaluate_once(&Check::FileContains { name, text: "hello from aura" }).is_ok());
        assert!(evaluate_once(&Check::FileContains { name, text: "something else" }).is_err());
        std::fs::remove_file(dir.join(name)).unwrap();
        assert!(evaluate_once(&Check::FileContains { name, text: "x" }).unwrap_err().contains("does not exist"));
    }

    #[test]
    fn the_report_table_lists_each_task() {
        let report = SuiteReport {
            model: "m".into(),
            suite: "basic".into(),
            passed: 1,
            total: 2,
            results: vec![
                TaskResult { id: "a".into(), passed: true, run_completed: true, steps_executed: 1, model_calls: 0, seconds: 2.5, message: "ok".into() },
                TaskResult { id: "b".into(), passed: false, run_completed: false, steps_executed: 3, model_calls: 7, seconds: 30.0, message: "nope".into() },
            ],
        };
        let table = report.table();
        assert!(table.contains("PASS") && table.contains("FAIL") && table.contains("1 / 2 passed"));
    }
}
