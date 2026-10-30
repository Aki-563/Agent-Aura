//! The run's control bus (HARNESS_COMPLETION_PLAN.md §H1): stop, pause/resume and steering, from
//! any source (UI buttons, global hotkeys, `aura-cli`, the control port, the scheduler).
//!
//! The central idea is the **epoch**: a counter that goes up on every stop, pause and steer. Each
//! model request remembers the epoch it was sent under; when the answer arrives (and again right
//! before input is injected) the epoch is compared, and an answer from an older epoch is dropped.
//! That closes the race where the user stops or steers at the moment the model answers.
//!
//! Stop works without the network: it only flips atomics and releases held input.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static EPOCH: AtomicU64 = AtomicU64::new(1);
static PAUSED: AtomicBool = AtomicBool::new(false);
static RUN_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set by the Ctrl+Alt+Space hotkey; the UI opens its steer box and clears it.
static STEER_BOX_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Pause the run (cut the current action) when the user touches the mouse or keyboard.
static YIELD_TO_USER: AtomicBool = AtomicBool::new(true);

struct Pending {
    steers: Vec<String>,
    pause_reason: String,
    paused_at: Option<Instant>,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending { steers: Vec::new(), pause_reason: String::new(), paused_at: None });

fn pending() -> std::sync::MutexGuard<'static, Pending> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

fn bump() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

/// The current epoch; remember it when sending a model request.
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

/// Whether a stop, pause or steer happened since `since`.
pub fn interrupted_since(since: u64) -> bool {
    epoch() != since
}

/// Called when a run starts: clears a pause or steer left over from the previous run.
pub fn begin_run() -> u64 {
    PAUSED.store(false, Ordering::SeqCst);
    let mut p = pending();
    p.steers.clear();
    p.pause_reason.clear();
    p.paused_at = None;
    drop(p);
    RUN_ACTIVE.store(true, Ordering::SeqCst);
    bump()
}

/// Called when a run ends for any reason.
pub fn end_run() {
    RUN_ACTIVE.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
}

pub fn run_active() -> bool {
    RUN_ACTIVE.load(Ordering::SeqCst)
}

/// Hard stop: every input loop sees it before its next event, the model request is abandoned,
/// speech stops, and held keys and buttons are released. Needs no network.
pub fn stop() {
    crate::input::killswitch::KillSwitch::trigger();
    PAUSED.store(false, Ordering::SeqCst);
    bump();
    crate::voice::stop_speaking();
    crate::input::send_input::InputActuator::release_all();
}

pub fn pause(reason: &str) {
    if !PAUSED.swap(true, Ordering::SeqCst) {
        let mut p = pending();
        p.pause_reason = reason.to_owned();
        p.paused_at = Some(Instant::now());
        drop(p);
        bump();
        crate::voice::stop_speaking();
    }
}

pub fn resume() {
    if PAUSED.swap(false, Ordering::SeqCst) {
        pending().paused_at = None;
        // A resume also changes the epoch: anything decided before the pause is stale.
        bump();
    }
}

pub fn toggle_pause() {
    if is_paused() {
        resume();
    } else {
        pause("paused by the Pause key");
    }
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

pub fn pause_reason() -> String {
    pending().pause_reason.clone()
}

/// How long the run has been paused.
pub fn paused_for() -> Option<Duration> {
    pending().paused_at.map(|t| t.elapsed())
}

/// A new instruction from the user mid-run. The in-flight model request is abandoned and the
/// next step sees the message. Several in a row are all kept, in order; the latest wins where
/// they disagree (the prompt says so).
pub fn steer(text: &str) {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect();
    if text.is_empty() {
        return;
    }
    pending().steers.push(text);
    bump();
    crate::voice::stop_speaking();
}

/// Steering messages not yet shown to the model.
pub fn take_steers() -> Vec<String> {
    std::mem::take(&mut pending().steers)
}

pub fn request_steer_box() {
    STEER_BOX_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn take_steer_box_request() -> bool {
    STEER_BOX_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn set_yield_to_user(on: bool) {
    YIELD_TO_USER.store(on, Ordering::SeqCst);
}

pub fn yield_to_user() -> bool {
    YIELD_TO_USER.load(Ordering::SeqCst) && std::env::var_os("AURA_NO_YIELD").is_none()
}

/// Whether input injection must stop before its next event (other than a hard stop): the run is
/// paused, or the user just touched the mouse or keyboard during a run.
pub fn input_should_yield() -> bool {
    if !run_active() {
        return false;
    }
    is_paused() || (yield_to_user() && crate::input::takeover::idle_ms() < 250)
}

/// Resolves when the run is stopped, paused, steered or otherwise moved past `since`.
pub async fn wait_for_interrupt(since: u64) {
    loop {
        if crate::input::killswitch::is_killed() || interrupted_since(since) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Where the control port's token is kept (readable only by the user's account, like the rest of
/// `%LOCALAPPDATA%`). A command must start with it, so another program on the machine that does
/// not know it cannot stop or steer a run.
pub fn token_path() -> std::path::PathBuf {
    crate::settings::path().with_file_name("control.token")
}

fn load_or_create_token() -> String {
    if let Ok(token) = std::fs::read_to_string(token_path()) {
        if token.trim().len() >= 16 {
            return token.trim().to_owned();
        }
    }
    let seed = format!("{:?}{}{}", Instant::now(), std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    use sha2::Digest;
    let token = format!("{:x}", sha2::Sha256::digest(seed.as_bytes()))[..32].to_owned();
    if let Some(parent) = token_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(token_path(), &token);
    token
}

/// Answers one control line: `<token> stop|pause|resume|status|steer <text>`.
pub fn handle_command(line: &str, token: &str) -> String {
    let mut parts = line.trim().splitn(3, ' ');
    if parts.next() != Some(token) {
        return "error: wrong or missing token".into();
    }
    match (parts.next().unwrap_or(""), parts.next().unwrap_or("")) {
        ("stop", _) => {
            stop();
            "ok: stopped".into()
        }
        ("pause", _) => {
            pause("paused from the control port");
            "ok: paused".into()
        }
        ("resume", _) => {
            resume();
            "ok: resumed".into()
        }
        ("steer", text) if !text.trim().is_empty() => {
            steer(text);
            resume();
            "ok: instruction sent".into()
        }
        ("status", _) => format!("ok: running={} paused={} epoch={}", run_active(), is_paused(), epoch()),
        (other, _) => format!("error: unknown command '{other}' (stop, pause, resume, steer <text>, status)"),
    }
}

/// Serves the control port on 127.0.0.1 (never on other interfaces) until the process ends.
pub async fn serve_port(port: u16) -> Result<u16, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("control port {port}: {e}"))?;
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let token = load_or_create_token();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        while let Ok((stream, _)) = listener.accept().await {
            let token = token.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                if let Ok(Ok(Some(line))) = tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await {
                    let reply = handle_command(&line, &token);
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                }
            });
        }
    });
    Ok(bound)
}

/// Client side: sends one command to a running Aura's control port.
pub fn send_command(port: u16, command: &str) -> Result<String, String> {
    use std::io::{BufRead, Write};
    let token = std::fs::read_to_string(token_path()).map_err(|_| "no control token yet: is an Aura run with a control port running?".to_owned())?;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("could not reach the control port {port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    stream.write_all(format!("{} {command}\n", token.trim()).as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    std::io::BufReader::new(stream).read_line(&mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim().to_owned())
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interruption_moves_the_epoch_so_old_answers_are_recognised() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let start = begin_run();
        assert!(!interrupted_since(start));
        steer("use the other tab");
        assert!(interrupted_since(start), "a steer makes the in-flight answer stale");
        let after_steer = epoch();
        pause("test");
        assert!(interrupted_since(after_steer));
        let paused_epoch = epoch();
        pause("again"); // pausing twice changes nothing
        assert_eq!(epoch(), paused_epoch);
        resume();
        assert!(interrupted_since(paused_epoch), "a resume also invalidates what was decided before");
        end_run();
    }

    #[test]
    fn steers_keep_their_order_and_are_taken_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_run();
        steer("go back");
        steer("   click the second   button instead ");
        steer("");
        assert_eq!(take_steers(), vec!["go back".to_owned(), "click the second button instead".to_owned()]);
        assert!(take_steers().is_empty());
        end_run();
    }

    #[test]
    fn control_commands_need_the_token() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_run();
        assert!(handle_command("stop", "secret-token-123456").starts_with("error"));
        assert!(handle_command("wrong pause", "secret-token-123456").starts_with("error"));
        assert_eq!(handle_command("secret-token-123456 pause", "secret-token-123456"), "ok: paused");
        assert!(is_paused());
        assert_eq!(handle_command("secret-token-123456 steer use the other tab", "secret-token-123456"), "ok: instruction sent");
        assert!(!is_paused(), "a steer resumes");
        assert_eq!(take_steers(), vec!["use the other tab".to_owned()]);
        assert!(handle_command("secret-token-123456 dance", "secret-token-123456").contains("unknown command"));
        end_run();
    }

    #[test]
    fn a_new_run_clears_a_leftover_pause_and_steers() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_run();
        pause("x");
        steer("left over");
        begin_run();
        assert!(!is_paused());
        assert!(take_steers().is_empty());
        assert!(input_should_yield() == (yield_to_user() && crate::input::takeover::idle_ms() < 250));
        end_run();
        assert!(!input_should_yield(), "no run: input is never held back");
    }
}
