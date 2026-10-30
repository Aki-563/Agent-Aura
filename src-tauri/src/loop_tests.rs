//! End-to-end tests of the agent loop against the scripted fake model.
//!
//! These run the real loop (real screen capture, HTTP, schema parsing, retries, completion
//! check, approval flow) but only ever script actions that send no input (`note`, `wait_settle`,
//! `done`), so they never move the mouse or press keys.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::agent::{
    execute_run, AgentRuntime, ApiProtocol, AutonomousResponse, Host, HostBackend, ProviderConfigInput,
};
use crate::mock_server::{replies, MockServer};

/// The loop uses process-wide state (kill switch, target window), so these tests run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct HeadlessHost {
    events: Arc<Mutex<Vec<(String, Value)>>>,
    answer: Option<bool>,
}

impl HostBackend for HeadlessHost {
    fn emit_value(&self, event: &str, payload: Value) {
        if let Ok(mut events) = self.events.lock() {
            events.push((event.to_owned(), payload));
        }
    }

    fn auto_answer(&self) -> Option<bool> {
        self.answer
    }
}

struct Outcome {
    response: AutonomousResponse,
    server: MockServer,
    events: Vec<(String, Value)>,
    data_root: PathBuf,
}

fn scratch_dir() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("aura-loop-test-{nanos}"))
}

async fn run(task: &str, max_steps: u32, script: Vec<(&str, String)>, answer: Option<bool>) -> Outcome {
    let _serial = SERIAL.lock().await;
    let server = MockServer::start(script).await.expect("mock server");
    let data_root = scratch_dir();
    let runtime = AgentRuntime::with_data_root(data_root.clone(), false).expect("runtime");
    runtime
        .set_provider(ProviderConfigInput {
            protocol: ApiProtocol::OpenAiCompatibleChat,
            base_url: server.base_url.clone(),
            model: "mock-model".to_owned(),
            api_key: String::new(),
        })
        .expect("provider");
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = Host::new(HeadlessHost { events: Arc::clone(&events), answer });
    let response = execute_run(&runtime, &host, task.to_owned(), max_steps).await.expect("run");
    let events = events.lock().unwrap().clone();
    Outcome { response, server, events, data_root }
}

fn note(text: &str) -> Value {
    json!({"type": "note", "text": text})
}

fn done(summary: &str) -> Value {
    json!({"type": "done", "summary": summary})
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_are_remembered_and_shown_in_later_prompts() {
    let outcome = run(
        "remember the order number then finish",
        5,
        vec![
            ("aura_proposal", replies::proposal(note("order 4411"), "save the number")),
            ("aura_proposal", replies::proposal(done("noted"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        None,
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 2);
    assert!(proposals[1].contains("YOUR SAVED NOTES") && proposals[1].contains("order 4411"));

    // The run left a readable trace on disk.
    let runs = outcome.data_root.join("runs");
    let run_dir = std::fs::read_dir(&runs).unwrap().flatten().next().expect("a run folder").path();
    let log = std::fs::read_to_string(run_dir.join("run.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"note\"") && log.contains("\"kind\":\"run_end\""));
    assert!(run_dir.join("step_001").join("obs.png").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_done_claim_the_screen_does_not_support_is_rejected_then_accepted() {
    let outcome = run(
        "finish when it is visible",
        6,
        vec![
            ("aura_proposal", replies::proposal(done("all good"), "claim done")),
            ("aura_proposal", replies::proposal(done("really done"), "claim again")),
            ("aura_done_check", replies::done_check(false, "the text is not visible")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        None,
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 2);
    assert!(proposals[1].contains("REJECTED") && proposals[1].contains("the text is not visible"));
    assert_eq!(outcome.server.requests_of("aura_done_check").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_done_claim_without_the_asked_text_on_screen_is_rejected_by_the_harness() {
    // Nothing is typed (the scripted model only claims done), so the exact text cannot be on screen.
    let outcome = run(
        "type the sentence zebra quokka narwhal in the box",
        6,
        vec![
            ("aura_proposal", replies::proposal(done("typed it"), "claim done")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        None,
    )
    .await;
    assert!(!outcome.response.completed, "a done claim without the text must not complete: {}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert!(proposals.len() >= 2);
    assert!(proposals[1].contains("zebra quokka narwhal") && proposals[1].contains("REJECTED"), "the model is told exactly what is missing");
    assert!(outcome.server.requests_of("aura_done_check").is_empty(), "the harness decided from the screen; the model's own check was never needed");
    assert!(outcome.response.message.contains("zebra quokka narwhal"), "{}", outcome.response.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn endless_unsupported_done_claims_end_the_run_honestly() {
    let outcome = run(
        "finish when it is visible",
        10,
        vec![
            ("aura_proposal", replies::proposal(done("done"), "claim")),
            ("aura_done_check", replies::done_check(false, "nothing changed")),
        ],
        None,
    )
    .await;
    assert!(!outcome.response.completed);
    assert!(outcome.response.message.contains("could not confirm"), "{}", outcome.response.message);
    assert_eq!(outcome.server.requests_of("aura_proposal").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_garbled_reply_is_retried_with_the_exact_error() {
    let outcome = run(
        "finish",
        5,
        vec![
            ("aura_proposal", "I think we should click somewhere".to_owned()),
            ("aura_proposal", replies::proposal(done("fine"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        None,
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 2);
    assert!(proposals[1].contains("RETRY ATTEMPT 1") && proposals[1].contains("did not contain a JSON object"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_risky_action_stops_the_run_after_the_model_tries_again() {
    let outcome = run(
        "look around the screen",
        5,
        vec![
            (
                "aura_proposal",
                replies::proposal_with_impact(json!({"type": "hotkey", "keys": ["ctrl", "shift", "delete"]}), "wipe it", "destructive"),
            ),
            ("aura_verification", replies::accept_verification()),
        ],
        Some(false),
    )
    .await;
    assert!(!outcome.response.completed);
    assert!(outcome.response.message.contains("declined"), "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 3, "the model gets two more chances after a refusal");
    assert!(proposals[1].contains("DECLINED"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_risky_action_goes_ahead() {
    let outcome = run(
        "look around the screen",
        5,
        vec![
            (
                "aura_proposal",
                replies::proposal_with_impact(json!({"type": "hotkey", "keys": ["shift"]}), "tap shift", "destructive"),
            ),
            ("aura_proposal", replies::proposal(done("ok"), "finish")),
            ("aura_verification", replies::accept_verification()),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        Some(true),
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    assert_eq!(outcome.response.steps_executed, 1);
    let phases: Vec<String> = outcome
        .events
        .iter()
        .filter_map(|(_, payload)| payload.get("phase").and_then(Value::as_str).map(str::to_owned))
        .collect();
    for expected in ["starting", "capturing", "acting", "settling", "result", "checking"] {
        assert!(phases.iter().any(|p| p == expected), "no `{expected}` phase in {phases:?}");
    }
}

/// Real desktop, scripted model: the eval harness opens Notepad (fast path, no model call for
/// that step), the fake model then reports done, and the independent check confirms Notepad is
/// in front. Cleanup must close only the window the task opened.
/// Run with: cargo test --release --lib live_eval_open_notepad -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_eval_open_notepad_with_a_scripted_model() {
    let _serial = SERIAL.lock().await;
    let server = MockServer::start(vec![
        ("aura_proposal", replies::proposal(done("Notepad is open"), "it is open")),
        ("aura_done_check", replies::done_check(true, "")),
    ])
    .await
    .expect("mock server");
    let runtime = AgentRuntime::with_data_root(scratch_dir(), false).expect("runtime");
    runtime
        .set_provider(ProviderConfigInput {
            protocol: ApiProtocol::OpenAiCompatibleChat,
            base_url: server.base_url.clone(),
            model: "mock-model".to_owned(),
            api_key: String::new(),
        })
        .expect("provider");
    let host = Host::new(HeadlessHost { events: Arc::new(Mutex::new(Vec::new())), answer: Some(false) });
    let before = crate::desktop::target::list_candidate_windows();
    let report = crate::eval::run_suite(&runtime, &host, "basic", Some("open-notepad"), "mock-model").await;
    println!("{}", report.table());
    let after = crate::desktop::target::list_candidate_windows();
    assert_eq!(report.total, 1);
    assert_eq!(report.passed, 1, "{}", report.table());
    assert_eq!(report.results[0].model_calls, 2, "one proposal plus one completion check");
    assert_eq!(before.len(), after.len(), "cleanup must leave exactly the windows that were open before");
    assert!(crate::eval::save_report(&runtime, &report).is_some());
}

// ---------------------------------------------------------------- stop, pause and steer (§H1)

/// One control action, fired `after_ms` after the `nth` request of `kind` reached the fake model.
struct ControlEvent {
    kind: &'static str,
    nth: usize,
    after_ms: u64,
    act: fn(),
}

/// Runs a task (holding the serial lock for the whole time, so control actions can never land
/// in another test's run) while a controller thread fires `events` relative to model requests.
/// Returns the outcome and when the last event fired.
async fn run_with_control(task: &str, max_steps: u32, script: Vec<(&str, String)>, events: Vec<ControlEvent>) -> (Outcome, Option<std::time::Instant>) {
    let _serial = SERIAL.lock().await;
    let server = MockServer::start(script).await.expect("mock server");
    let watcher = server.watcher();
    let controller = std::thread::spawn(move || {
        let mut fired = None;
        for event in events {
            if !watcher.wait_for(event.kind, event.nth, std::time::Duration::from_secs(60)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(event.after_ms));
            (event.act)();
            fired = Some(std::time::Instant::now());
        }
        fired
    });
    let data_root = scratch_dir();
    let runtime = AgentRuntime::with_data_root(data_root.clone(), false).expect("runtime");
    runtime
        .set_provider(ProviderConfigInput { protocol: ApiProtocol::OpenAiCompatibleChat, base_url: server.base_url.clone(), model: "mock-model".to_owned(), api_key: String::new() })
        .expect("provider");
    let events_log = Arc::new(Mutex::new(Vec::new()));
    let host = Host::new(HeadlessHost { events: Arc::clone(&events_log), answer: None });
    let response = execute_run(&runtime, &host, task.to_owned(), max_steps).await.expect("run");
    let fired = controller.join().ok().flatten();
    let events = events_log.lock().unwrap().clone();
    (Outcome { response, server, events, data_root }, fired)
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_during_a_hanging_model_call_ends_the_run_within_half_a_second() {
    // The model never answers (like an unreachable model machine); stop must not wait for it.
    let (outcome, fired) = run_with_control(
        "anything",
        3,
        vec![("aura_proposal", "@hang".to_owned())],
        vec![ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 300, act: crate::control::stop }],
    )
    .await;
    let after_stop = fired.expect("stop fired").elapsed();
    assert!(outcome.response.stopped, "{}", outcome.response.message);
    assert_eq!(outcome.response.steps_executed, 0);
    assert!(after_stop < std::time::Duration::from_millis(500), "the run ended {after_stop:?} after stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_steer_during_the_model_call_drops_the_old_answer_and_reaches_the_next_prompt() {
    fn steer() {
        crate::control::steer("use the other tab instead");
    }
    let (outcome, _) = run_with_control(
        "remember something then finish",
        6,
        vec![
            // The first answer would arrive after the steer: it must never be executed.
            ("aura_proposal", format!("@delay:1500|{}", replies::proposal(note("STALE NOTE"), "old plan"))),
            ("aura_proposal", replies::proposal(note("fresh note"), "new plan")),
            ("aura_proposal", replies::proposal(done("done"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        vec![ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 300, act: steer }],
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert!(proposals.len() >= 2);
    assert!(proposals[1..].iter().all(|p| p.contains("use the other tab instead")), "every later prompt carries the instruction");
    assert!(proposals.iter().all(|p| !p.contains("STALE NOTE")), "the stale answer was never executed (its note never shows up)");
    assert!(proposals.last().unwrap().contains("fresh note"), "the new plan ran");
    let done_checks = outcome.server.requests_of("aura_done_check");
    assert!(done_checks.iter().any(|p| p.contains("use the other tab instead")), "the completion check judges the amended task");
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_then_resume_looks_again_and_tells_the_model() {
    fn pause() {
        crate::control::pause("test pause");
    }
    fn resume() {
        crate::control::resume();
    }
    let (outcome, _) = run_with_control(
        "remember then finish",
        6,
        vec![
            ("aura_proposal", format!("@delay:1500|{}", replies::proposal(note("STALE"), "old"))),
            ("aura_proposal", replies::proposal(done("done"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        vec![
            ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 300, act: pause },
            ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 1500, act: resume },
        ],
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 2, "one dropped request, one after the resume");
    assert!(proposals[1].contains("the run was paused for"), "the model is told about the pause");
    assert!(proposals.iter().all(|p| !p.contains("STALE")), "the answer from before the pause was dropped");
}

/// T3: a long pause (10 minutes by default; `AURA_PAUSE_SECS` shortens it). While paused the run
/// asks the model nothing, and after the resume it carries on with a fresh look. Run with
/// `cargo test --lib long_pause -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "takes 10 minutes"]
async fn long_pause_makes_no_requests_and_resumes_cleanly() {
    static AT_PAUSE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static AT_RESUME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn pause() {
        crate::control::pause("long pause test");
        // Give the run a moment to settle into the paused state, then start measuring.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        AT_PAUSE.store(process_cpu_ms(), std::sync::atomic::Ordering::SeqCst);
    }
    fn resume() {
        AT_RESUME.store(process_cpu_ms(), std::sync::atomic::Ordering::SeqCst);
        crate::control::resume();
    }
    let seconds: u64 = std::env::var("AURA_PAUSE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    let cpu_before = process_cpu_ms();
    let started = std::time::Instant::now();
    if std::env::var("AURA_PAUSE_TRACE").is_ok() {
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            eprintln!("t={:>3}s cpu={} ms", started.elapsed().as_secs(), process_cpu_ms().saturating_sub(cpu_before));
        });
    }
    let (outcome, _) = run_with_control(
        "remember then finish",
        6,
        vec![
            ("aura_proposal", format!("@delay:1500|{}", replies::proposal(note("STALE"), "old"))),
            ("aura_proposal", replies::proposal(done("done"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        vec![
            ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 300, act: pause },
            ControlEvent { kind: "aura_proposal", nth: 1, after_ms: seconds * 1000, act: resume },
        ],
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    assert!(started.elapsed() >= std::time::Duration::from_secs(seconds));
    assert_eq!(outcome.server.requests_of("aura_proposal").len(), 2, "no model request while paused");
    // Idle means idle: while paused the process may use only a little CPU (polling every 100 ms).
    use std::sync::atomic::Ordering::SeqCst;
    let cpu_used = AT_RESUME.load(SeqCst).saturating_sub(AT_PAUSE.load(SeqCst));
    assert!(cpu_used < 200 + seconds * 10, "{cpu_used} ms of CPU while paused for {seconds} s");
    let _ = cpu_before;
    crate::testlog::record("pause", "long", json!({"seconds": seconds, "cpu_ms": cpu_used}));
}

fn process_cpu_ms() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let (mut create, mut exit, mut kernel, mut user) = (std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>(), std::mem::zeroed::<FILETIME>());
        GetProcessTimes(GetCurrentProcess(), &mut create, &mut exit, &mut kernel, &mut user);
        let to_u64 = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
        (to_u64(kernel) + to_u64(user)) / 10_000
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_racing_the_model_answer_never_lets_the_answer_run() {
    // The answer arrives 400 ms after the request; stop lands from 60 ms before to 30 ms after.
    for after_ms in [340u64, 370, 390, 400, 410, 430] {
        let (outcome, _) = run_with_control(
            "race",
            2,
            vec![("aura_proposal", format!("@delay:400|{}", replies::proposal(json!({"type": "wait_settle", "seconds": 0.2}), "x")))],
            vec![ControlEvent { kind: "aura_proposal", nth: 1, after_ms, act: crate::control::stop }],
        )
        .await;
        assert!(outcome.response.stopped, "stop at {after_ms} ms: the run must end stopped, got {}", outcome.response.message);
        assert_eq!(outcome.response.steps_executed, 0, "stop at {after_ms} ms: the answer must not run");
    }
}

// ---------------------------------------------------------------- API failures (§H6)

fn quick_backoff() {
    std::env::set_var("AURA_BACKOFF_SCALE", "0.02");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_server_is_retried_and_the_action_runs_exactly_once() {
    quick_backoff();
    let (outcome, _) = run_with_control(
        "remember then finish",
        5,
        vec![
            ("aura_proposal", "@status:503|".to_owned()),
            ("aura_proposal", "@status:503|".to_owned()),
            ("aura_proposal", replies::proposal(note("only once"), "note")),
            ("aura_proposal", replies::proposal(done("done"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        vec![],
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    let proposals = outcome.server.requests_of("aura_proposal");
    assert_eq!(proposals.len(), 4, "2 failed tries, the note, the finish");
    // The note step ran once: the prompt after it lists it exactly once.
    let last = proposals.last().unwrap();
    // Three places show the note (trajectory action, its effect line, the saved notes); a second
    // execution of the note would add a second trajectory entry and a second line in each.
    assert_eq!(last.matches("only once").count(), 3, "the retries did not run the note again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_is_retried() {
    quick_backoff();
    let (outcome, _) = run_with_control(
        "finish",
        3,
        vec![
            ("aura_proposal", "@drop".to_owned()),
            ("aura_proposal", replies::proposal(done("done"), "finish")),
            ("aura_done_check", replies::done_check(true, "")),
        ],
        vec![],
    )
    .await;
    assert!(outcome.response.completed, "{}", outcome.response.message);
    assert_eq!(outcome.server.requests_of("aura_proposal").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_request_is_not_retried_as_if_the_server_were_busy() {
    quick_backoff();
    let (outcome, _) = run_with_control("x", 2, vec![("aura_proposal", "@status:400|".to_owned())], vec![]).await;
    assert!(!outcome.response.completed);
    let tries = outcome.server.requests_of("aura_proposal").len();
    assert!(tries <= 4, "a 400 gets the normal feedback retries only, not 4x4 transient ones: {tries}");
    assert!(outcome.response.message.contains("400") || outcome.response.message.to_lowercase().contains("stopped"), "{}", outcome.response.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_during_a_backoff_pause_ends_the_run_at_once() {
    std::env::set_var("AURA_BACKOFF_SCALE", "1.0");
    let (outcome, fired) = run_with_control(
        "x",
        3,
        vec![("aura_proposal", "@status:503|".to_owned())],
        vec![ControlEvent { kind: "aura_proposal", nth: 1, after_ms: 150, act: crate::control::stop }],
    )
    .await;
    let after = fired.expect("stop fired").elapsed();
    assert!(outcome.response.stopped, "{}", outcome.response.message);
    assert!(after < std::time::Duration::from_millis(500), "stopped {after:?} after the press, during the 0.5 s backoff");
    assert_eq!(outcome.server.requests_of("aura_proposal").len(), 1, "no further request after the stop");
}
