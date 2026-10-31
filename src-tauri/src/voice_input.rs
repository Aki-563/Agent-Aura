//! Voice input: the user speaks a task (or, during a run, a correction) in Tamil, Tanglish,
//! English or another Indian language; Sarvam AI turns it into English text for the model.
//!
//! * The microphone is recorded here with the Windows wave-in API (16 kHz, mono, 16-bit), only
//!   while the user has the mic on. A small local voice detector decides when speech starts and
//!   ends, so recording stops by itself after a pause, and silence is never sent anywhere.
//! * Speech is sent to Sarvam AI's speech-to-text in `translate` mode, which always answers in
//!   English (`POST https://api.sarvam.ai/speech-to-text`, header `api-subscription-key`, form
//!   fields `file`, `model`, `mode`). One request takes at most 30 s of audio.
//! * The Sarvam key is DPAPI-encrypted in `voice_input.json` under the data folder, like the
//!   model keys.
//! * During a run, speech that starts pauses the agent (and its step reader) at once; the English
//!   text then goes to the model as a steering message and the run resumes. What the step reader
//!   itself says is not taken for the user's voice.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Recording format: what Sarvam handles best and what keeps uploads small.
pub const SAMPLE_RATE: u32 = 16_000;
/// Frames spent measuring the room's noise before listening for speech (300 ms).
const CALIBRATION_FRAMES: u32 = 10;
/// Length of one analysis frame (30 ms).
const FRAME: usize = (SAMPLE_RATE as usize * 30) / 1000;
/// Sarvam's limit is 30 s per request; stop a little before it.
const MAX_SECONDS: f32 = 28.0;
/// Speech kept from just before the detector fired, so the first syllable is not clipped.
const PRE_ROLL_FRAMES: usize = 10;

const SARVAM_URL: &str = "https://api.sarvam.ai/speech-to-text";
/// The current model first; the previous one if an account or region does not have it yet.
const SARVAM_MODELS: [&str; 2] = ["saaras:v4", "saaras:v3"];
const KEY_FILE: &str = "voice_input.json";

static LISTENING: AtomicBool = AtomicBool::new(false);
/// True while the recorded speech is with Sarvam (the page shows "translating").
static TRANSLATING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// Input level 0..1000 for the microphone animation.
static LEVEL: AtomicU32 = AtomicU32::new(0);

// ---- Voice detection (pure, testable) ------------------------------------------------------------

/// Root-mean-square level of 16-bit samples, 0.0 to 1.0.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[derive(Debug, Clone, Copy)]
pub struct DetectorSettings {
    /// Silence after speech that ends the recording.
    pub end_silence_ms: u32,
    /// Give up when nobody has said anything for this long.
    pub no_speech_ms: u32,
}

impl Default for DetectorSettings {
    fn default() -> Self {
        Self { end_silence_ms: 1300, no_speech_ms: 9000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Waiting,
    SpeechStarted,
    Speaking,
    /// Speech ended (a long enough pause).
    Finished,
    /// Nothing was said in time.
    NothingSaid,
}

/// Decides, frame by frame, when speech starts and ends. The first 300 ms measure the room (a fan
/// or an air conditioner does not count as speech); the threshold then follows that noise floor,
/// with a fixed minimum.
pub struct Detector {
    settings: DetectorSettings,
    noise: f32,
    frames: u32,
    loud_run: u32,
    quiet_run: u32,
    speaking: bool,
}

impl Detector {
    pub fn new(settings: DetectorSettings) -> Self {
        Self { settings, noise: 0.004, frames: 0, loud_run: 0, quiet_run: 0, speaking: false }
    }

    pub fn has_speech(&self) -> bool {
        self.speaking
    }

    fn ms(frames: u32) -> u32 {
        frames * 30
    }

    /// `level` is the frame's RMS; `ignore` marks frames to treat as silence (Aura's own voice).
    pub fn push(&mut self, level: f32, ignore: bool) -> Verdict {
        self.frames += 1;
        let level = if ignore { 0.0 } else { level };
        if self.frames <= CALIBRATION_FRAMES {
            self.noise = if self.frames == 1 { level.max(0.0005) } else { self.noise + (level.max(0.0005) - self.noise) / self.frames as f32 };
            return Verdict::Waiting;
        }
        // Capped, so a noisy room still hears normal speech.
        let threshold = (self.noise * 3.2).clamp(0.012, 0.05);
        let loud = level > threshold;
        if !loud {
            // The floor adapts quickly downwards and slowly upwards.
            let rate = if level < self.noise { 0.2 } else { 0.02 };
            self.noise += (level.max(0.0005) - self.noise) * rate;
        }
        if loud {
            self.loud_run += 1;
            self.quiet_run = 0;
        } else {
            self.quiet_run += 1;
            self.loud_run = 0;
        }
        if !self.speaking {
            // About 120 ms of sound in a row is speech; a click or a cough is not.
            if self.loud_run >= 4 {
                self.speaking = true;
                return Verdict::SpeechStarted;
            }
            if Self::ms(self.frames) >= self.settings.no_speech_ms {
                return Verdict::NothingSaid;
            }
            return Verdict::Waiting;
        }
        if Self::ms(self.quiet_run) >= self.settings.end_silence_ms {
            return Verdict::Finished;
        }
        Verdict::Speaking
    }
}

/// A 16-bit mono PCM WAV file.
pub fn wav_bytes(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// What one listening turn produced.
pub struct Recording {
    pub samples: Vec<i16>,
    pub had_speech: bool,
}

/// Runs frames through the detector and keeps the speech (with a little audio before it).
/// `frames` yields (samples, ignore); `on_speech` is called once when speech starts.
pub fn collect_speech(frames: impl Iterator<Item = (Vec<i16>, bool)>, settings: DetectorSettings, stop: &AtomicBool, mut on_speech: impl FnMut()) -> Recording {
    let mut detector = Detector::new(settings);
    let mut pre_roll: std::collections::VecDeque<Vec<i16>> = std::collections::VecDeque::new();
    let mut kept: Vec<i16> = Vec::new();
    let max_samples = (MAX_SECONDS * SAMPLE_RATE as f32) as usize;
    for (frame, ignore) in frames {
        let level = rms(&frame);
        LEVEL.store((level * 4000.0).min(1000.0) as u32, Ordering::Relaxed);
        let verdict = detector.push(level, ignore);
        match verdict {
            Verdict::Waiting => {
                pre_roll.push_back(frame);
                if pre_roll.len() > PRE_ROLL_FRAMES {
                    pre_roll.pop_front();
                }
            }
            Verdict::SpeechStarted => {
                on_speech();
                for f in pre_roll.drain(..) {
                    kept.extend_from_slice(&f);
                }
                kept.extend_from_slice(&frame);
            }
            Verdict::Speaking => kept.extend_from_slice(&frame),
            Verdict::Finished | Verdict::NothingSaid => break,
        }
        if kept.len() >= max_samples || stop.load(Ordering::SeqCst) {
            break;
        }
    }
    LEVEL.store(0, Ordering::Relaxed);
    Recording { had_speech: detector.has_speech(), samples: kept }
}

// ---- Microphone ------------------------------------------------------------------------------------

/// 30 ms frames from the default microphone until the iterator is dropped.
struct Microphone {
    handle: windows_sys::Win32::Media::Audio::HWAVEIN,
    headers: Vec<Box<windows_sys::Win32::Media::Audio::WAVEHDR>>,
    buffers: Vec<Vec<i16>>,
    next: usize,
    pending: std::collections::VecDeque<i16>,
}

const BUFFER_SAMPLES: usize = FRAME * 3; // 90 ms per driver buffer
const BUFFERS: usize = 8;

impl Microphone {
    fn open() -> Result<Self, String> {
        use windows_sys::Win32::Media::Audio::*;
        unsafe {
            if waveInGetNumDevs() == 0 {
                return Err("No microphone was found. Plug one in or turn it on in Windows sound settings.".into());
            }
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: 1,
                nSamplesPerSec: SAMPLE_RATE,
                nAvgBytesPerSec: SAMPLE_RATE * 2,
                nBlockAlign: 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut handle: HWAVEIN = std::ptr::null_mut();
            let result = waveInOpen(&mut handle, WAVE_MAPPER, &format, 0, 0, CALLBACK_NULL);
            if result != 0 || handle.is_null() {
                return Err(format!(
                    "Windows did not open the microphone (error {result}). Check Settings > Privacy & security > Microphone: \"Let desktop apps access your microphone\" must be on."
                ));
            }
            let mut mic = Self { handle, headers: Vec::new(), buffers: Vec::new(), next: 0, pending: Default::default() };
            for _ in 0..BUFFERS {
                let mut buffer = vec![0i16; BUFFER_SAMPLES];
                let mut header: Box<WAVEHDR> = Box::new(std::mem::zeroed());
                header.lpData = buffer.as_mut_ptr() as *mut u8;
                header.dwBufferLength = (BUFFER_SAMPLES * 2) as u32;
                waveInPrepareHeader(handle, &mut *header, std::mem::size_of::<WAVEHDR>() as u32);
                waveInAddBuffer(handle, &mut *header, std::mem::size_of::<WAVEHDR>() as u32);
                mic.buffers.push(buffer);
                mic.headers.push(header);
            }
            if waveInStart(handle) != 0 {
                return Err("Windows did not start the microphone.".into());
            }
            Ok(mic)
        }
    }

    /// The next 30 ms frame, waiting for the driver as needed (None after 2 s without audio).
    fn frame(&mut self) -> Option<Vec<i16>> {
        use windows_sys::Win32::Media::Audio::*;
        let waited = Instant::now();
        while self.pending.len() < FRAME {
            let header = &mut self.headers[self.next];
            // The driver fills the header from its own thread; read it fresh every time.
            let flags = unsafe { std::ptr::addr_of!(header.dwFlags).read_unaligned() };
            if flags & WHDR_DONE == 0 {
                if waited.elapsed() > Duration::from_secs(2) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let recorded = unsafe { std::ptr::addr_of!(header.dwBytesRecorded).read_unaligned() } as usize / 2;
            self.pending.extend(&self.buffers[self.next][..recorded.min(BUFFER_SAMPLES)]);
            header.dwFlags &= !WHDR_DONE;
            header.dwBytesRecorded = 0;
            unsafe { waveInAddBuffer(self.handle, &mut **header, std::mem::size_of::<WAVEHDR>() as u32) };
            self.next = (self.next + 1) % BUFFERS;
        }
        Some(self.pending.drain(..FRAME).collect())
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        use windows_sys::Win32::Media::Audio::*;
        unsafe {
            waveInReset(self.handle);
            for header in &mut self.headers {
                waveInUnprepareHeader(self.handle, &mut **header, std::mem::size_of::<WAVEHDR>() as u32);
            }
            waveInClose(self.handle);
        }
    }
}

/// Records one spoken turn from the microphone (blocking).
fn record_turn(settings: DetectorSettings, on_speech: impl FnMut()) -> Result<Recording, String> {
    let mut mic = Microphone::open()?;
    let stop = &STOP;
    let frames = std::iter::from_fn(move || {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        // Aura's own step reader coming out of the speakers is not the user.
        mic.frame().map(|f| (f, crate::voice::is_speaking()))
    });
    Ok(collect_speech(frames, settings, stop, on_speech))
}

/// `aura-cli mic-check`: records one turn and reports what the detector heard, without sending
/// anything anywhere. Returns (speech heard, seconds kept, loudest level 0..1000).
pub fn mic_check() -> Result<(bool, f32, u32), String> {
    STOP.store(false, Ordering::SeqCst);
    let peak = std::sync::Arc::new(AtomicU32::new(0));
    let watcher = {
        let peak = peak.clone();
        std::thread::spawn(move || {
            for _ in 0..400 {
                peak.fetch_max(LEVEL.load(Ordering::Relaxed), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(25));
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
            }
        })
    };
    let recording = record_turn(DetectorSettings::default(), || println!("speech started"));
    STOP.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let recording = recording?;
    Ok((recording.had_speech, recording.samples.len() as f32 / SAMPLE_RATE as f32, peak.load(Ordering::Relaxed)))
}

// ---- Sarvam AI -----------------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedKey {
    #[serde(default)]
    sarvam_key_encrypted: String,
}

fn data_root() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("AgentAura")
}

fn key_path() -> std::path::PathBuf {
    data_root().join(KEY_FILE)
}

fn saved_key() -> Option<String> {
    let saved: SavedKey = serde_json::from_str(&std::fs::read_to_string(key_path()).ok()?).ok()?;
    crate::vault::decrypt(&saved.sarvam_key_encrypted).ok().filter(|k| !k.is_empty())
}

fn save_key(key: &str) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&SavedKey { sarvam_key_encrypted: crate::vault::encrypt(key)? }).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_root()).map_err(|e| e.to_string())?;
    std::fs::write(key_path(), text).map_err(|e| format!("could not save the Sarvam key: {e}"))
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().connect_timeout(Duration::from_secs(8)).timeout(Duration::from_secs(45)).build().expect("http client"))
}

fn sarvam_url() -> String {
    std::env::var("AURA_SARVAM_URL").unwrap_or_else(|_| SARVAM_URL.to_owned())
}

/// The English text Sarvam made of `wav`, and the language it heard (e.g. "ta-IN").
pub async fn translate_speech(key: &str, wav: Vec<u8>) -> Result<(String, Option<String>), String> {
    let mut last_error = String::new();
    for model in SARVAM_MODELS {
        let part = reqwest::multipart::Part::bytes(wav.clone()).file_name("speech.wav").mime_str("audio/wav").map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part).text("model", model).text("mode", "translate");
        let response = client()
            .post(sarvam_url())
            .header("api-subscription-key", key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| if e.is_timeout() { "Sarvam AI did not answer in time. Check the internet connection.".to_owned() } else { format!("Could not reach Sarvam AI: {e}") })?;
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        match status {
            200 => {
                let value: Value = serde_json::from_str(&body).map_err(|_| "Sarvam AI answered in an unexpected way.".to_owned())?;
                let text = value.get("transcript").and_then(Value::as_str).unwrap_or("").trim().to_owned();
                let language = value.get("language_code").and_then(Value::as_str).map(str::to_owned);
                return Ok((text, language));
            }
            401 | 403 => return Err("Sarvam AI did not accept the API key. Check it in Settings.".into()),
            429 => return Err("Sarvam AI says the key is out of credits or over its rate limit.".into()),
            400 | 422 if body.to_lowercase().contains("model") => {
                // This model is not available to the key: try the previous one.
                last_error = format!("Sarvam AI refused model {model}: {}", body.chars().take(200).collect::<String>());
            }
            _ => return Err(format!("Sarvam AI answered HTTP {status}: {}", body.chars().take(200).collect::<String>())),
        }
    }
    Err(last_error)
}

// ---- App commands ------------------------------------------------------------------------------

#[tauri::command]
pub fn voice_input_status() -> Value {
    json!({
        "hasKey": saved_key().is_some(),
        "listening": LISTENING.load(Ordering::SeqCst),
        "phase": if TRANSLATING.load(Ordering::SeqCst) { "translating" } else if LISTENING.load(Ordering::SeqCst) { "listening" } else { "idle" },
        "level": LEVEL.load(Ordering::Relaxed),
    })
}

/// Checks the key with Sarvam (half a second of silence: a cheap request) and saves it.
#[tauri::command]
pub async fn set_sarvam_key(key: String) -> Result<(), String> {
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("Paste the API key from the Sarvam AI dashboard.".into());
    }
    translate_speech(&key, wav_bytes(&vec![0i16; SAMPLE_RATE as usize / 2], SAMPLE_RATE)).await?;
    save_key(&key)
}

#[tauri::command]
pub fn clear_sarvam_key() {
    let _ = std::fs::remove_file(key_path());
}

/// Ends the current listening turn: `cancel` drops what was heard, otherwise it is translated.
#[tauri::command]
pub fn stop_listening(cancel: bool) {
    if cancel {
        CANCEL.store(true, Ordering::SeqCst);
    }
    STOP.store(true, Ordering::SeqCst);
}

/// Clears the "listening" flag however the turn ends.
struct ListeningGuard;

impl Drop for ListeningGuard {
    fn drop(&mut self) {
        LEVEL.store(0, Ordering::Relaxed);
        TRANSLATING.store(false, Ordering::SeqCst);
        LISTENING.store(false, Ordering::SeqCst);
    }
}

/// Listens for one spoken turn and returns it in English.
///
/// `purpose` is "task" (the home screen: the text goes into the task box) or "steer" (during a
/// run: speech pauses the agent, and the text is sent to it as a correction; nothing heard
/// resumes it). Returns `{text, language}`; `text` is empty when nothing was said.
#[tauri::command]
pub async fn listen_and_translate(purpose: String) -> Result<Value, String> {
    let key = saved_key().ok_or("Add a Sarvam AI API key in Settings to speak tasks.")?;
    if LISTENING.swap(true, Ordering::SeqCst) {
        return Err("Already listening.".into());
    }
    let _guard = ListeningGuard;
    STOP.store(false, Ordering::SeqCst);
    CANCEL.store(false, Ordering::SeqCst);
    let steer = purpose == "steer";
    let paused_for_speech = std::sync::Arc::new(AtomicBool::new(false));
    let paused = paused_for_speech.clone();
    let recording = tokio::task::spawn_blocking(move || {
        record_turn(DetectorSettings::default(), move || {
            if steer && crate::control::run_active() {
                // The user is talking to Aura: stop acting and stop talking until they are done.
                crate::voice::stop_speaking();
                crate::control::pause("the user is speaking");
                paused.store(true, Ordering::SeqCst);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    let resume_if_paused = || {
        if paused_for_speech.load(Ordering::SeqCst) {
            crate::control::resume();
        }
    };
    if CANCEL.load(Ordering::SeqCst) || !recording.had_speech || recording.samples.len() < SAMPLE_RATE as usize / 4 {
        resume_if_paused();
        return Ok(json!({"text": "", "language": null}));
    }
    TRANSLATING.store(true, Ordering::SeqCst);
    let result = translate_speech(&key, wav_bytes(&recording.samples, SAMPLE_RATE)).await;
    let (text, language) = match result {
        Ok(found) => found,
        Err(error) => {
            resume_if_paused();
            return Err(error);
        }
    };
    if steer && !text.is_empty() && crate::control::run_active() {
        // Sent like a typed correction: it reaches the model with what is already done.
        crate::control::steer(&text);
        crate::control::resume();
    } else {
        resume_if_paused();
    }
    Ok(json!({"text": text, "language": language}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, amplitude: f32) -> Vec<(Vec<i16>, bool)> {
        (0..frames)
            .map(|f| {
                let samples = (0..FRAME).map(|i| ((((f * FRAME + i) as f32) * 0.07).sin() * amplitude * 32767.0) as i16).collect();
                (samples, false)
            })
            .collect()
    }

    fn quiet(frames: usize) -> Vec<(Vec<i16>, bool)> {
        tone(frames, 0.002)
    }

    #[test]
    fn speech_between_pauses_is_kept_with_a_little_lead_in() {
        // 0.6 s quiet, 1.5 s speech, 2 s quiet: the turn ends 1.3 s into the last pause.
        let frames: Vec<_> = [quiet(20), tone(50, 0.3), quiet(67)].concat();
        let mut started = 0;
        let rec = collect_speech(frames.into_iter(), DetectorSettings::default(), &AtomicBool::new(false), || started += 1);
        assert!(rec.had_speech);
        assert_eq!(started, 1, "the speech callback fires once");
        let seconds = rec.samples.len() as f32 / SAMPLE_RATE as f32;
        assert!((2.6..=3.2).contains(&seconds), "speech + lead-in + trailing pause, got {seconds:.2} s");
    }

    #[test]
    fn silence_and_a_single_click_are_not_speech() {
        let mut frames = quiet(150);
        frames[40] = tone(1, 0.5).remove(0); // one 30 ms click
        let settings = DetectorSettings { end_silence_ms: 1300, no_speech_ms: 3000 };
        let rec = collect_speech(frames.into_iter(), settings, &AtomicBool::new(false), || panic!("no speech"));
        assert!(!rec.had_speech && rec.samples.is_empty());
    }

    #[test]
    fn a_steady_fan_noise_raises_the_bar_but_speech_still_counts() {
        let frames: Vec<_> = [tone(60, 0.02), tone(40, 0.25), tone(60, 0.02)].concat();
        let rec = collect_speech(frames.into_iter(), DetectorSettings::default(), &AtomicBool::new(false), || {});
        assert!(rec.had_speech, "speech over the noise is found");
        let fan_only = collect_speech(tone(200, 0.02).into_iter(), DetectorSettings { end_silence_ms: 1300, no_speech_ms: 5000 }, &AtomicBool::new(false), || {});
        assert!(!fan_only.had_speech, "the fan alone is not speech");
    }

    #[test]
    fn aura_s_own_voice_is_ignored() {
        let frames: Vec<_> = tone(80, 0.3).into_iter().map(|(s, _)| (s, true)).collect();
        let rec = collect_speech(frames.into_iter(), DetectorSettings { end_silence_ms: 1300, no_speech_ms: 2000 }, &AtomicBool::new(false), || {});
        assert!(!rec.had_speech);
    }

    #[test]
    fn wav_files_have_a_correct_header() {
        let wav = wav_bytes(&[0, 1000, -1000], 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 44 + 6);
    }

    /// The two Sarvam tests point the app at a fake server through one environment variable.
    static SARVAM_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A one-shot HTTP server standing in for Sarvam: checks the request and answers `reply`.
    async fn fake_sarvam(status: u16, reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/speech-to-text", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let response = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    #[tokio::test]
    async fn speech_goes_to_sarvam_in_translate_mode_and_comes_back_in_english() {
        let _env = SARVAM_ENV.lock().await;
        let (url, server) = fake_sarvam(200, r#"{"request_id":"r1","transcript":"Open Notepad and type hello","language_code":"ta-IN"}"#).await;
        std::env::set_var("AURA_SARVAM_URL", &url);
        let (text, language) = translate_speech("test-key", wav_bytes(&[0; 1600], SAMPLE_RATE)).await.unwrap();
        std::env::remove_var("AURA_SARVAM_URL");
        let request = server.await.unwrap();
        assert_eq!((text.as_str(), language.as_deref()), ("Open Notepad and type hello", Some("ta-IN")));
        assert!(request.starts_with("POST /speech-to-text"));
        assert!(request.to_lowercase().contains("api-subscription-key: test-key"));
        assert!(request.contains("name=\"mode\"\r\n\r\ntranslate"), "translate mode");
        assert!(request.contains("name=\"model\"\r\n\r\nsaaras:v4"));
        assert!(request.contains("filename=\"speech.wav\""));
    }

    #[tokio::test]
    async fn a_wrong_key_says_so() {
        let _env = SARVAM_ENV.lock().await;
        let (url, _server) = fake_sarvam(403, r#"{"error":{"message":"Invalid API key"}}"#).await;
        std::env::set_var("AURA_SARVAM_URL", &url);
        let error = translate_speech("bad", wav_bytes(&[0; 1600], SAMPLE_RATE)).await.unwrap_err();
        std::env::remove_var("AURA_SARVAM_URL");
        assert!(error.contains("did not accept the API key"), "{error}");
    }
}
