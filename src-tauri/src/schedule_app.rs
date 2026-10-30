//! Schedules inside the app: the same store and scheduler the `aura-cli daemon` uses, started when
//! the app opens, plus the commands the schedule screen calls. Runs started by a schedule never
//! ask the user anything (risky actions are declined), because nobody may be at the keyboard.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::agent::{execute_run, unattended_host, AgentRuntime};
use crate::schedule::{self, Missed, Overlap, RunReport, Runner, Schedule, Store, When};

struct Shared {
    store: Arc<Mutex<Store>>,
    changed: Arc<tokio::sync::Notify>,
    root: std::path::PathBuf,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn data_root() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura")
}

pub fn start(app: AppHandle) {
    let root = data_root();
    let shared = SHARED.get_or_init(|| Shared { store: Arc::new(Mutex::new(Store::load(&root))), changed: Arc::new(tokio::sync::Notify::new()), root: root.clone() });
    let (store, changed) = (Arc::clone(&shared.store), Arc::clone(&shared.changed));
    let runner: Runner = Arc::new(move |s: Schedule| {
        let app = app.clone();
        Box::pin(async move {
            if tokio::task::spawn_blocking(crate::system_state::secure_desktop_active).await.unwrap_or(false) {
                return RunReport { summary: "skipped: the PC was locked".to_owned(), screen_hash: None };
            }
            let Some(window) = app.get_webview_window("main") else {
                return RunReport { summary: "skipped: no window".to_owned(), screen_hash: None };
            };
            let state = app.state::<AgentRuntime>();
            let host = unattended_host(window);
            let summary = match execute_run(&state, &host, schedule::task_with_state(&s), 40).await {
                Ok(r) => r.message,
                Err(e) => format!("failed: {e}"),
            };
            RunReport { summary, screen_hash: tokio::task::spawn_blocking(crate::schedule_cli::screen_hash).await.ok().flatten() }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = RunReport> + Send>>
    });
    let hash: Arc<dyn Fn() -> Option<u64> + Send + Sync> = Arc::new(crate::schedule_cli::screen_hash);
    tauri::async_runtime::spawn(schedule::run_forever(root, store, changed, runner, hash));
}

fn describe(s: &Schedule) -> Value {
    let when = match &s.when {
        When::Every { seconds } => format!("every {}", if seconds % 3600 == 0 { format!("{} h", seconds / 3600) } else if seconds % 60 == 0 { format!("{} min", seconds / 60) } else { format!("{seconds} s") }),
        When::DailyAt { times } => format!("daily at {}", times.join(", ")),
    };
    json!({
        "id": s.id, "task": s.task, "when": when, "enabled": s.enabled,
        "nextInSeconds": s.next_run_ms.saturating_sub(schedule::now_ms()) / 1000,
        "overlap": format!("{:?}", s.overlap), "missed": format!("{:?}", s.missed),
        "onlyIfChanged": s.only_if_changed, "lastSummary": s.last_summary, "lastRunMs": s.last_run_ms,
    })
}

fn with_store<T>(f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let shared = SHARED.get().ok_or("The scheduler has not started yet.")?;
    let mut store = shared.store.lock().map_err(|_| "The schedule list is locked.".to_owned())?;
    let result = f(&mut store)?;
    store.save(&shared.root)?;
    shared.changed.notify_one();
    Ok(result)
}

#[tauri::command]
pub fn list_schedules() -> Vec<Value> {
    SHARED.get().and_then(|s| s.store.lock().ok().map(|store| store.schedules.iter().map(describe).collect())).unwrap_or_default()
}

/// `mode` is "every" (value like "30m", "2h") or "daily" (value like "09:00,17:30").
#[tauri::command]
pub fn add_schedule(task: String, mode: String, value: String, overlap: String, missed: String, only_if_changed: bool) -> Result<String, String> {
    if task.trim().is_empty() {
        return Err("Write what the schedule should do.".to_owned());
    }
    let when = match mode.as_str() {
        "every" => When::Every { seconds: schedule::parse_interval(&value).ok_or("Use an interval like 30s, 5m or 2h.")? },
        "daily" => When::DailyAt { times: value.split(',').map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect() },
        _ => return Err("Choose \"every\" or \"daily\".".to_owned()),
    };
    let overlap = match overlap.as_str() {
        "queue" => Overlap::QueueOne,
        "cancel" => Overlap::CancelPrevious,
        _ => Overlap::Skip,
    };
    let missed = if missed == "skip" { Missed::Skip } else { Missed::RunOnce };
    with_store(|store| store.add(task.trim(), when, overlap, missed, only_if_changed, schedule::now_ms(), schedule::local_offset_minutes()))
}

#[tauri::command]
pub fn remove_schedule(id: String) -> Result<(), String> {
    with_store(|store| if store.remove(&id) { Ok(()) } else { Err("No such schedule.".to_owned()) })
}

#[tauri::command]
pub fn set_schedule_enabled(id: String, enabled: bool) -> Result<(), String> {
    with_store(|store| {
        let tz = schedule::local_offset_minutes();
        let s = store.schedules.iter_mut().find(|s| s.id == id).ok_or("No such schedule.")?;
        s.enabled = enabled;
        if enabled {
            // Count from now: a schedule switched off for a week does not "catch up" on switching on.
            s.next_run_ms = schedule::next_run_after(&s.when, s.anchor_ms, schedule::now_ms(), tz).unwrap_or(u64::MAX);
        }
        Ok(())
    })
}
