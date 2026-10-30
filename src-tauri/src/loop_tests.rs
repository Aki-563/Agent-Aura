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
                replies::proposal_with_impact(json!({"type": "wait_settle", "max_timeout_ms": 100}), "wipe it", "destructive"),
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
                replies::proposal_with_impact(json!({"type": "wait_settle", "max_timeout_ms": 100}), "pause", "destructive"),
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
