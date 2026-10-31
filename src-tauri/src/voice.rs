//! Voice announcements (HARNESS_COMPLETION_PLAN.md §H8): before each action the harness speaks
//! the model's `voice_command` (or a phrase made from the action) with the offline Windows speech
//! engine (SAPI 5). Nothing is sent over the network.
//!
//! * Non-blocking by default: `announce` returns at once; `speak_then_act` waits (bounded).
//! * No backlog: at most one sentence waits. A newer sentence replaces the waiting one, and cuts
//!   the one being spoken once that has played `min_play_ms` (stale news is not worth hearing).
//! * `stop_speaking` (called by stop, pause and steer) silences it immediately.
//! * Any failure (no audio device, no voices, COM error) disables voice for the process and is
//!   logged; it never reaches the agent loop.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::input::ActionPrimitive;
use crate::settings::VoiceSettings;

/// What the speaking thread is asked to do.
enum Command {
    Speak { text: String, done: Option<Sender<()>> },
    Stop,
}

/// The speech engine, behind a trait so tests can use a recording fake.
pub trait TtsBackend: Send {
    fn configure(&mut self, settings: &VoiceSettings) -> Result<(), String>;
    /// Starts speaking `text` without waiting; `purge` cuts anything playing first.
    fn speak(&mut self, text: &str, purge: bool) -> Result<(), String>;
    fn stop(&mut self);
    fn is_speaking(&self) -> bool;
    /// Waits up to `timeout` for the current sentence to finish.
    fn wait_done(&mut self, timeout: Duration);
}

static SENDER: Mutex<Option<Sender<Command>>> = Mutex::new(None);
static DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn sender() -> Option<Sender<Command>> {
    if DISABLED.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let mut guard = SENDER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("aura-voice".into()).spawn(move || match sapi::SapiVoice::new() {
            Ok(backend) => run(Box::new(backend), rx),
            Err(error) => {
                log::warn!("voice disabled: {error}");
                DISABLED.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        if spawned.is_err() {
            DISABLED.store(true, std::sync::atomic::Ordering::SeqCst);
            return None;
        }
        *guard = Some(tx);
    }
    guard.clone()
}

/// The speaking loop: always acts on the newest sentence, never builds a queue.
fn run(mut backend: Box<dyn TtsBackend>, commands: Receiver<Command>) {
    let mut configured: Option<VoiceSettings> = None;
    let mut started: Option<Instant> = None;
    while let Ok(first) = commands.recv() {
        // Drain whatever else is waiting: only the latest sentence and any stop matter.
        let mut latest = first;
        let mut stop_seen = false;
        let mut waiters: Vec<Sender<()>> = Vec::new();
        while let Ok(next) = commands.try_recv() {
            if let Command::Speak { done: Some(done), .. } = &latest {
                waiters.push(done.clone());
            }
            if matches!(next, Command::Stop) {
                stop_seen = true;
            }
            latest = next;
        }
        let settings = crate::settings::get().voice;
        if configured.as_ref() != Some(&settings) {
            if let Err(error) = backend.configure(&settings) {
                log::warn!("voice settings not applied: {error}");
            }
            configured = Some(settings.clone());
        }
        match latest {
            Command::Stop => {
                backend.stop();
                started = None;
            }
            Command::Speak { text, done } => {
                if stop_seen {
                    backend.stop();
                }
                // Let a sentence that just started play its first words before it is cut.
                if let Some(since) = started.filter(|_| backend.is_speaking()) {
                    let min_play = Duration::from_millis(settings.min_play_ms);
                    if since.elapsed() < min_play {
                        std::thread::sleep(min_play - since.elapsed());
                    }
                }
                let result = backend.speak(&text, true);
                started = Some(Instant::now());
                if let Err(error) = result {
                    log::warn!("could not speak: {error}");
                }
                if let Some(done) = done {
                    backend.wait_done(Duration::from_secs(8));
                    let _ = done.send(());
                }
            }
        }
        for waiter in waiters {
            let _ = waiter.send(());
        }
    }
}

/// Until when (Unix ms) the step reader is probably still talking: an estimate from the length of
/// the last sentence. Voice input treats sound during that time as Aura's own voice, not the user.
static SPEAKING_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether Aura is (probably) speaking a step right now.
pub fn is_speaking() -> bool {
    now_ms() < SPEAKING_UNTIL.load(std::sync::atomic::Ordering::Relaxed)
}

/// About 75 ms per character at the default rate, plus the room's echo.
fn note_speaking(text: &str) {
    let estimate = 400 + 75 * text.chars().count() as u64;
    SPEAKING_UNTIL.store(now_ms() + estimate, std::sync::atomic::Ordering::Relaxed);
}

/// Speaks `text` before an action. Returns at once unless `speak_then_act` is set, in which case
/// it waits for the sentence (at most 8 s, and never past a stop).
pub fn announce(text: &str) {
    let settings = crate::settings::get().voice;
    let text = text.trim();
    if !settings.enabled || text.is_empty() {
        return;
    }
    let Some(tx) = sender() else { return };
    note_speaking(text);
    if settings.speak_then_act {
        let (done_tx, done_rx) = mpsc::channel();
        if tx.send(Command::Speak { text: text.to_owned(), done: Some(done_tx) }).is_ok() {
            let deadline = Instant::now() + Duration::from_secs(8);
            while Instant::now() < deadline && !crate::input::killswitch::is_killed() {
                if done_rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                    break;
                }
            }
        }
    } else {
        let _ = tx.send(Command::Speak { text: text.to_owned(), done: None });
    }
}

/// Silences speech immediately (stop, pause and steer call this).
pub fn stop_speaking() {
    SPEAKING_UNTIL.store(0, std::sync::atomic::Ordering::Relaxed);
    if DISABLED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(tx) = SENDER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(Command::Stop);
    }
}

/// What to say when the model gave no `voice_command`: a short phrase built from the action.
pub fn fallback_phrase(action: &ActionPrimitive, target: &str) -> String {
    use ActionPrimitive as A;
    let on = |verb: &str, x: i32, y: i32| {
        let target = target.trim();
        if target.is_empty() || target.len() > 60 { format!("{verb} at {x}, {y}") } else { format!("{verb} {target}") }
    };
    match action {
        A::Click { x, y, clicks, button, .. } => {
            let verb = match (clicks, button) {
                (2, _) => "Double-clicking",
                (3, _) => "Triple-clicking",
                (_, crate::input::MouseButton::Right) => "Right-clicking",
                _ => "Clicking",
            };
            on(verb, *x, *y)
        }
        A::DoubleClick { x, y, .. } => on("Double-clicking", *x, *y),
        A::MouseMove { x, y } | A::Hover { x, y, .. } => on("Hovering over", *x, *y),
        A::MouseDown { x, y, .. } => on("Pressing", *x, *y),
        A::MouseUp { .. } => "Releasing the mouse button".into(),
        A::DragAndDrop { from_x, from_y, .. } => on("Dragging", *from_x, *from_y),
        A::DrawPath { .. } => "Drawing".into(),
        A::TypeText { text, .. } | A::TypeWith { text, .. } => {
            if text.chars().count() <= 24 { format!("Typing {text}") } else { "Typing the text".into() }
        }
        A::Hotkey { keys } => format!("Pressing {}", keys.join(" ")),
        A::KeyPress { key } | A::HoldKey { key, .. } | A::KeyDown { key } => format!("Pressing {key}"),
        A::KeyUp { key } => format!("Releasing {key}"),
        A::Scroll { clicks, .. } => format!("Scrolling {}", if *clicks > 0 { "up" } else { "down" }),
        A::ScrollFine { delta, hold_keys, .. } => {
            if hold_keys.iter().any(|k| k.eq_ignore_ascii_case("ctrl")) { if *delta > 0 { "Zooming in".into() } else { "Zooming out".into() } } else { format!("Scrolling {}", if *delta > 0 { "up" } else { "down" }) }
        }
        A::OpenApp { name } => format!("Opening {name}"),
        A::FocusWindow { name } => format!("Switching to {name}"),
        A::OpenUrl { url } => format!("Opening {}", url.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or("the page")),
        A::WaitSettle { .. } => "Waiting".into(),
        A::Note { .. } => "Noting that down".into(),
        A::AskUser { .. } => "I need your help".into(),
        A::Done { .. } => "Task finished".into(),
        A::Fail { .. } => "I could not finish the task".into(),
        A::Zoom { .. } => "Looking closer".into(),
        A::Observe {} => "Looking at the screen".into(),
        A::ClipboardRead {} => "Reading the clipboard".into(),
        A::ClipboardWrite { .. } => "Copying to the clipboard".into(),
        A::WindowAction { command, .. } => format!("{} the window", match command {
            crate::input::WindowCommand::Minimize => "Minimizing",
            crate::input::WindowCommand::Maximize => "Maximizing",
            crate::input::WindowCommand::Restore => "Restoring",
            crate::input::WindowCommand::Close => "Closing",
            crate::input::WindowCommand::Move => "Moving",
            crate::input::WindowCommand::Resize => "Resizing",
            crate::input::WindowCommand::Snap => "Snapping",
        }),
        A::LayoutWindows { .. } => "Arranging the windows".into(),
        A::SwitchDesktop { direction } => format!("Switching desktop to the {direction}"),
        A::Touch { gesture, .. } => format!("Touch {}", format!("{gesture:?}").to_lowercase()),
    }
}

/// The sentence to speak for a step: the model's `voice_command`, else the fallback, with the
/// window name in front when several windows are being worked in ("Notepad: typing the heading").
pub fn sentence(voice_command: Option<&str>, action: &ActionPrimitive, target: &str, window_label: Option<&str>) -> String {
    let core = voice_command.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).unwrap_or_else(|| fallback_phrase(action, target));
    let core: String = core.chars().take(120).collect();
    match window_label.map(str::trim).filter(|w| !w.is_empty()) {
        Some(window) => {
            let mut chars = core.chars();
            let lowered = chars.next().map(|c| c.to_lowercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            format!("{window}: {lowered}")
        }
        None => core,
    }
}

mod sapi {
    use super::{TtsBackend, VoiceSettings};
    use std::time::Duration;
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::Media::Speech::{ISpObjectTokenCategory, ISpVoice, SpObjectTokenCategory, SpVoice, SPCAT_VOICES, SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK, SPRS_IS_SPEAKING, SPVOICESTATUS};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};

    pub struct SapiVoice {
        voice: ISpVoice,
    }

    // The voice lives on the one thread that created it (an STA); the box only moves it there.
    unsafe impl Send for SapiVoice {}

    impl SapiVoice {
        pub fn new() -> Result<Self, String> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let voice: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL).map_err(|e| format!("Windows speech is not available: {e}"))?;
                Ok(Self { voice })
            }
        }

        fn select_voice(&self, wanted: &str) -> Result<(), String> {
            let wanted = wanted.trim().to_lowercase();
            if wanted.is_empty() {
                return Ok(());
            }
            unsafe {
                let category: ISpObjectTokenCategory = CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
                category.SetId(SPCAT_VOICES, false).map_err(|e| e.to_string())?;
                let tokens = category.EnumTokens(PCWSTR::null(), PCWSTR::null()).map_err(|e| e.to_string())?;
                let mut count = 0u32;
                tokens.GetCount(&mut count).map_err(|e| e.to_string())?;
                for index in 0..count {
                    let Ok(token) = tokens.Item(index) else { continue };
                    let name = token.GetStringValue(PCWSTR::null()).ok().and_then(|p| p.to_string().ok()).unwrap_or_default();
                    if name.to_lowercase().contains(&wanted) {
                        return self.voice.SetVoice(&token).map_err(|e| e.to_string());
                    }
                }
            }
            Err(format!("no installed voice matches '{wanted}'"))
        }
    }

    impl TtsBackend for SapiVoice {
        fn configure(&mut self, settings: &VoiceSettings) -> Result<(), String> {
            unsafe {
                self.voice.SetVolume(settings.volume.min(100)).map_err(|e| e.to_string())?;
                self.voice.SetRate(settings.rate.clamp(-10, 10)).map_err(|e| e.to_string())?;
            }
            self.select_voice(&settings.voice)
        }

        fn speak(&mut self, text: &str, purge: bool) -> Result<(), String> {
            let flags = SPF_ASYNC.0 | SPF_IS_NOT_XML.0 | if purge { SPF_PURGEBEFORESPEAK.0 } else { 0 };
            let text = HSTRING::from(text);
            unsafe { self.voice.Speak(&text, flags as u32, None) }.map_err(|e| e.to_string())
        }

        fn stop(&mut self) {
            unsafe {
                let _ = self.voice.Speak(PCWSTR::null(), SPF_PURGEBEFORESPEAK.0 as u32, None);
            }
        }

        fn is_speaking(&self) -> bool {
            let mut status = SPVOICESTATUS::default();
            unsafe { self.voice.GetStatus(&mut status, std::ptr::null_mut()).is_ok() && status.dwRunningState == SPRS_IS_SPEAKING.0 as u32 }
        }

        fn wait_done(&mut self, timeout: Duration) {
            unsafe {
                let _ = self.voice.WaitUntilDone(timeout.as_millis().min(u32::MAX as u128) as u32);
            }
        }
    }

    #[allow(dead_code)]
    fn _assert_interface(voice: &ISpVoice) -> windows::core::Result<ISpVoice> {
        voice.cast()
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Arc;

    /// Records what would have been spoken and when, with a fixed speaking time per sentence.
    #[derive(Clone, Default)]
    pub struct Recorder {
        pub log: Arc<Mutex<Vec<(Instant, String)>>>,
        pub stops: Arc<Mutex<u32>>,
        pub speaking_until: Arc<Mutex<Option<Instant>>>,
        pub fail: bool,
    }

    impl TtsBackend for Recorder {
        fn configure(&mut self, _: &VoiceSettings) -> Result<(), String> {
            Ok(())
        }
        fn speak(&mut self, text: &str, _purge: bool) -> Result<(), String> {
            if self.fail {
                return Err("no audio device".into());
            }
            self.log.lock().unwrap().push((Instant::now(), text.to_owned()));
            *self.speaking_until.lock().unwrap() = Some(Instant::now() + Duration::from_millis(400));
            Ok(())
        }
        fn stop(&mut self) {
            *self.stops.lock().unwrap() += 1;
            *self.speaking_until.lock().unwrap() = None;
        }
        fn is_speaking(&self) -> bool {
            self.speaking_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
        }
        fn wait_done(&mut self, timeout: Duration) {
            let until = *self.speaking_until.lock().unwrap();
            if let Some(until) = until {
                std::thread::sleep(until.saturating_duration_since(Instant::now()).min(timeout));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Recorder;
    use super::*;

    fn start(recorder: Recorder) -> Sender<Command> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run(Box::new(recorder), rx));
        tx
    }

    /// Real Windows speech (audible): run with `cargo test --lib real_speech -- --ignored`.
    /// Checks the engine starts a sentence, finishes it, and a stop cuts a long one quickly.
    #[test]
    #[ignore = "speaks out loud"]
    fn real_speech_starts_finishes_and_stops() {
        let mut voice = super::sapi::SapiVoice::new().expect("Windows speech is available");
        voice.configure(&VoiceSettings { volume: 60, ..Default::default() }).expect("configure");
        voice.speak("Agent Aura voice check.", true).expect("speak");
        let begun = Instant::now();
        while !voice.is_speaking() && begun.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(voice.is_speaking(), "speech did not start within 2 s (no audio device or voice?)");
        voice.wait_done(Duration::from_secs(8));
        assert!(!voice.is_speaking(), "the sentence did not finish");
        voice.speak("This is a long sentence that should be cut off by the stop command long before it ends, because it keeps going and going.", true).expect("speak long");
        std::thread::sleep(Duration::from_millis(500));
        let stopped = Instant::now();
        voice.stop();
        while voice.is_speaking() && stopped.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!voice.is_speaking() && stopped.elapsed() < Duration::from_millis(500), "stop took {:?}", stopped.elapsed());
    }

    #[test]
    fn the_model_sentence_is_spoken_and_a_phrase_is_made_when_it_is_missing() {
        let click = ActionPrimitive::Click { x: 842, y: 517, button: Default::default(), clicks: 1, hold_keys: vec![] };
        assert_eq!(sentence(Some("Clicking the Save button"), &click, "", None), "Clicking the Save button");
        assert_eq!(sentence(None, &click, "", None), "Clicking at 842, 517");
        assert_eq!(sentence(None, &click, "Save button", None), "Clicking Save button");
        assert_eq!(sentence(Some("  "), &click, "", Some("Notepad")), "Notepad: clicking at 842, 517");
        assert_eq!(sentence(Some("Typing the heading"), &click, "", Some("Notepad")), "Notepad: typing the heading");
    }

    #[test]
    fn a_burst_of_sentences_never_builds_a_backlog() {
        let recorder = Recorder::default();
        let log = recorder.log.clone();
        let tx = start(recorder);
        for i in 0..20 {
            tx.send(Command::Speak { text: format!("action {i}"), done: None }).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1500));
        let spoken: Vec<String> = log.lock().unwrap().iter().map(|(_, t)| t.clone()).collect();
        assert!(spoken.len() <= 3, "20 sentences in a burst must collapse, got {spoken:?}");
        assert_eq!(spoken.last().map(String::as_str), Some("action 19"), "the newest sentence is the one heard");
    }

    #[test]
    fn stop_silences_speech_at_once() {
        let recorder = Recorder::default();
        let (stops, until) = (recorder.stops.clone(), recorder.speaking_until.clone());
        let tx = start(recorder);
        tx.send(Command::Speak { text: "a long sentence".into(), done: None }).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let asked = Instant::now();
        tx.send(Command::Stop).unwrap();
        while *stops.lock().unwrap() == 0 && asked.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(asked.elapsed() < Duration::from_millis(50), "stop took {:?}", asked.elapsed());
        assert!(until.lock().unwrap().is_none());
    }

    #[test]
    fn a_broken_speech_engine_does_not_reach_the_caller() {
        let recorder = Recorder { fail: true, ..Default::default() };
        let tx = start(recorder);
        tx.send(Command::Speak { text: "x".into(), done: None }).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        tx.send(Command::Speak { text: "y".into(), done: Some(done_tx) }).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(2)).is_ok(), "the waiter is released even when speaking fails");
        // The public entry points never panic, even with voice muted (cargo test) or unavailable.
        announce("hello");
        stop_speaking();
    }
}
