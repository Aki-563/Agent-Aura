//! `aura-cli schedule add|list|remove` and `aura-cli daemon`: the command-line side of the
//! scheduler (schedule.rs). The daemon sleeps between runs; nothing is captured and no model is
//! called while idle.

use std::sync::{Arc, Mutex};

use crate::agent::{execute_run, AgentRuntime, Host};
use crate::schedule::{self, Missed, Overlap, RunReport, Runner, Schedule, Store, When};

fn data_root() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura")
}

/// Returns the process exit code.
pub fn schedule_command(args: &[String]) -> i32 {
    let root = data_root();
    let mut store = Store::load(&root);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("list") | None => {
            if store.schedules.is_empty() {
                println!("no schedules");
            }
            for s in &store.schedules {
                let when = match &s.when {
                    When::Every { seconds } => format!("every {seconds} s"),
                    When::DailyAt { times } => format!("daily at {}", times.join(", ")),
                };
                let next_in = s.next_run_ms.saturating_sub(schedule::now_ms()) / 1000;
                println!(
                    "{}  {}  next in {} s  overlap={:?} missed={:?}{}  task: {}",
                    s.id, when, next_in, s.overlap, s.missed, if s.only_if_changed { " only-if-changed" } else { "" }, s.task
                );
            }
            0
        }
        Some("add") => {
            let Some(task) = value("--task") else {
                eprintln!("usage: aura-cli schedule add --task <text> (--every 5m | --at 09:00,17:30) [--overlap skip|queue|cancel] [--missed run|skip] [--only-if-changed]");
                return 2;
            };
            let when = match (value("--every"), value("--at")) {
                (Some(every), None) => match schedule::parse_interval(&every) {
                    Some(seconds) => When::Every { seconds },
                    None => {
                        eprintln!("--every takes 30s, 5m, 2h or a number of minutes");
                        return 2;
                    }
                },
                (None, Some(at)) => When::DailyAt { times: at.split(',').map(|t| t.trim().to_owned()).collect() },
                _ => {
                    eprintln!("give exactly one of --every and --at");
                    return 2;
                }
            };
            let overlap = match value("--overlap").as_deref() {
                Some("queue") => Overlap::QueueOne,
                Some("cancel") => Overlap::CancelPrevious,
                _ => Overlap::Skip,
            };
            let missed = if value("--missed").as_deref() == Some("skip") { Missed::Skip } else { Missed::RunOnce };
            let only_if_changed = args.iter().any(|a| a == "--only-if-changed");
            match store.add(&task, when, overlap, missed, only_if_changed, schedule::now_ms(), schedule::local_offset_minutes()) {
                Ok(id) => match store.save(&root) {
                    Ok(()) => {
                        println!("added {id}; a running `aura-cli daemon` picks it up within a minute");
                        0
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        1
                    }
                },
                Err(error) => {
                    eprintln!("{error}");
                    2
                }
            }
        }
        Some("remove") => match args.get(1) {
            Some(id) if store.remove(id) => {
                let _ = store.save(&root);
                println!("removed {id}");
                0
            }
            _ => {
                eprintln!("no such schedule");
                1
            }
        },
        Some(other) => {
            eprintln!("unknown schedule command '{other}' (add, list, remove)");
            2
        }
    }
}

/// A hash of the text on screen (read locally with UI Automation and OCR; no model involved).
pub fn screen_hash() -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut text = crate::agent::visible_text_snapshot().ok()?;
    text.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    Some(hasher.finish())
}

/// Serves the saved schedules until the process is stopped.
pub async fn run_daemon(runtime: AgentRuntime, host: Host) {
    let root = data_root();
    let store = Arc::new(Mutex::new(Store::load(&root)));
    let changed = Arc::new(tokio::sync::Notify::new());
    let runtime = Arc::new(runtime);
    let host = Arc::new(host);
    let runner: Runner = {
        let (runtime, host) = (Arc::clone(&runtime), Arc::clone(&host));
        Arc::new(move |s: Schedule| {
            let (runtime, host) = (Arc::clone(&runtime), Arc::clone(&host));
            Box::pin(async move {
                println!("[schedule {}] starting: {}", s.id, s.task);
                let outcome = execute_run(&runtime, &host, schedule::task_with_state(&s), 40).await;
                let summary = match outcome {
                    Ok(r) => r.message,
                    Err(e) => format!("failed: {e}"),
                };
                println!("[schedule {}] finished: {summary}", s.id);
                RunReport { summary, screen_hash: tokio::task::spawn_blocking(screen_hash).await.ok().flatten() }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = RunReport> + Send>>
        })
    };
    // Pick up `aura-cli schedule add/remove` made while the daemon runs (checked once a minute).
    {
        let (store, changed, root) = (Arc::clone(&store), Arc::clone(&changed), root.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                let fresh = Store::load(&root);
                if let Ok(mut current) = store.lock() {
                    let ids = |s: &Store| s.schedules.iter().map(|x| x.id.clone()).collect::<Vec<_>>();
                    if ids(&fresh) != ids(&current) {
                        *current = fresh;
                        changed.notify_one();
                    }
                }
            }
        });
    }
    println!(
        "schedules: {} (Ctrl+C to quit). Idle between runs: nothing is captured and the model is not called.",
        store.lock().map(|s| s.schedules.len()).unwrap_or(0)
    );
    let hash: Arc<dyn Fn() -> Option<u64> + Send + Sync> = Arc::new(screen_hash);
    schedule::run_forever(root, store, changed, runner, hash).await;
}
