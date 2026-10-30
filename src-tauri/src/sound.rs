//! Start, finish and stop sounds for a run. Nothing is read from disk: the short sounds are
//! synthesised once into 16-bit mono WAV data and played with `PlaySoundW` straight from memory,
//! asynchronously, so a run never waits for audio.
//!
//! Set `AURA_MUTE=1` to silence them.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// A task is starting: two soft rising notes.
    Start,
    /// The task finished: a gentle three-note rise with a faint sparkle.
    Finished,
    /// The run ended without finishing (stopped, failed, step limit): two soft falling notes.
    Stopped,
}

const SAMPLE_RATE: u32 = 44_100;

/// One soft, glassy note: a sine with a quiet octave and a faint bell partial, a rounded 14 ms
/// attack and a long exponential tail, mixed into `out`.
fn add_note(out: &mut [f32], frequency: f32, start_secs: f32, length_secs: f32, volume: f32) {
    let first = (start_secs * SAMPLE_RATE as f32) as usize;
    let count = (length_secs * SAMPLE_RATE as f32) as usize;
    for i in 0..count {
        let index = first + i;
        if index >= out.len() {
            break;
        }
        let t = i as f32 / SAMPLE_RATE as f32;
        // Raised-cosine attack: no click, no hard edge.
        let attack = if t < 0.014 { 0.5 - 0.5 * (std::f32::consts::PI * t / 0.014).cos() } else { 1.0 };
        let decay = (-t * 4.2 / length_secs.max(0.05)).exp();
        let phase = 2.0 * std::f32::consts::PI * frequency * t;
        // The bell partial dies away much faster than the body of the note.
        let shimmer = 0.06 * (2.76 * phase).sin() * (-t * 18.0).exp();
        let tone = phase.sin() + 0.12 * (2.0 * phase).sin() + shimmer;
        out[index] += tone * attack * decay * volume;
    }
}

/// Builds the WAV file bytes (RIFF header + 16-bit mono PCM) for a sound.
pub fn synthesize(sound: Sound) -> Vec<u8> {
    // Soft two- and three-note chimes in the spirit of a phone's charging sound, not a doorbell.
    let (length, notes): (f32, Vec<(f32, f32, f32, f32)>) = match sound {
        // (frequency Hz, start s, length s, volume)
        Sound::Start => (0.9, vec![(1046.50, 0.00, 0.50, 0.55), (1567.98, 0.085, 0.65, 0.42)]),
        Sound::Finished => (1.3, vec![
            (1046.50, 0.00, 0.45, 0.45),
            (1318.51, 0.09, 0.45, 0.40),
            (1567.98, 0.18, 0.80, 0.40),
            (2093.00, 0.27, 0.85, 0.16),
        ]),
        Sound::Stopped => (0.85, vec![(880.00, 0.00, 0.45, 0.45), (659.25, 0.11, 0.62, 0.40)]),
    };
    let total = (length * SAMPLE_RATE as f32) as usize;
    let mut dry = vec![0.0f32; total];
    for (frequency, start, len, volume) in notes {
        add_note(&mut dry, frequency, start, len, volume);
    }
    // A little room: a few quiet early reflections soften the tail.
    let mut mix = dry.clone();
    for (delay_ms, gain) in [(37.0f32, 0.16f32), (71.0, 0.10), (113.0, 0.06), (167.0, 0.035)] {
        let delay = (delay_ms / 1000.0 * SAMPLE_RATE as f32) as usize;
        for i in delay..total {
            mix[i] += dry[i - delay] * gain;
        }
    }
    // Gentle one-pole low-pass (about 6 kHz) takes the edge off the highest partials.
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 6000.0 / SAMPLE_RATE as f32).exp();
    let mut state = 0.0f32;
    for value in mix.iter_mut() {
        state += alpha * (*value - state);
        *value = state;
    }
    // Normalise to a soft peak, with a long fade-out so there is never a click.
    let peak = mix.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(0.0001);
    let fade = (0.12 * SAMPLE_RATE as f32) as usize;
    let mut pcm: Vec<u8> = Vec::with_capacity(total * 2);
    for (i, value) in mix.iter().enumerate() {
        let tail = total - i;
        let gain = if tail < fade { (tail as f32 / fade as f32).powi(2) } else { 1.0 };
        let sample = (value / peak * 0.45 * gain * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

fn cached(sound: Sound) -> &'static [u8] {
    static START: OnceLock<Vec<u8>> = OnceLock::new();
    static FINISHED: OnceLock<Vec<u8>> = OnceLock::new();
    static STOPPED: OnceLock<Vec<u8>> = OnceLock::new();
    let cell = match sound {
        Sound::Start => &START,
        Sound::Finished => &FINISHED,
        Sound::Stopped => &STOPPED,
    };
    cell.get_or_init(|| synthesize(sound)).as_slice()
}

fn muted() -> bool {
    std::env::var_os("AURA_MUTE").is_some_and(|value| value != "0")
}

/// Plays a sound in the background. Returns immediately; does nothing when muted or under test.
pub fn play(sound: Sound) {
    if cfg!(test) || muted() {
        return;
    }
    let data = cached(sound);
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    // The data is a `'static` buffer, so it stays valid for the whole asynchronous playback.
    unsafe {
        PlaySoundW(data.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(wav: &[u8]) -> Vec<i16> {
        wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    #[test]
    fn every_sound_is_a_valid_short_wav() {
        for sound in [Sound::Start, Sound::Finished, Sound::Stopped] {
            let wav = synthesize(sound);
            assert_eq!(&wav[0..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), SAMPLE_RATE);
            let declared = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
            assert_eq!(declared, wav.len() - 44, "{sound:?}: data length must match the header");
            let seconds = samples(&wav).len() as f32 / SAMPLE_RATE as f32;
            assert!((0.5..1.6).contains(&seconds), "{sound:?} lasts {seconds}s");
        }
    }

    #[test]
    fn sounds_are_audible_but_never_clip_and_end_silently() {
        for sound in [Sound::Start, Sound::Finished, Sound::Stopped] {
            let pcm = samples(&synthesize(sound));
            let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 10_000, "{sound:?} is too quiet (peak {peak})");
            assert!(peak < 26_000, "{sound:?} is too loud (peak {peak})");
            assert!(pcm.last().unwrap().unsigned_abs() < 50, "{sound:?} must fade to silence, no click");
        }
    }

    #[test]
    fn the_three_sounds_differ() {
        assert_ne!(synthesize(Sound::Start), synthesize(Sound::Finished));
        assert_ne!(synthesize(Sound::Finished), synthesize(Sound::Stopped));
    }
}
