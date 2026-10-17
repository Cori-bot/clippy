//! Generates the chime Clippy plays when it saves a clip.
//!
//! Run once; its output is committed. It exists so the sound in the repo has a
//! recipe rather than a provenance — nobody has to wonder where a binary blob
//! came from or what licence it carries.
//!
//! Usage: cargo run -p clippy-core --example make-chime -- crates/clippy-daemon/assets/clip.wav
//!
//! The sound is two soft plucked notes, a fifth apart and rising, a small
//! "got it". An earlier version was two plain sine waves at 880 and 1318 Hz,
//! which is a test tone: bright, thin, and piercing over game audio. This one
//! keeps the notes low, gives each a quick round attack and a warm decay in
//! which the upper partials die first (as in a struck wooden bar), and adds a
//! short soft room so it does not stop dead. Its peak is about -7 dBFS,
//! because it is heard at whatever volume the game is already at.

use std::f32::consts::TAU;

/// Mono at 44.1 kHz. The notes are low, but the pluck and the softened room
/// carry detail up to a few kHz that 22 kHz would have thinned.
const RATE: u32 = 44_100;
const SECONDS: f32 = 0.5;

/// C5, then G5: a rising fifth, the most open and least nagging interval.
const NOTES: [(f32, f32); 2] = [(523.25, 0.0), (783.99, 0.085)];

/// Upper partials as (multiple, level, how much faster than the fundamental
/// it decays). They are what make a pluck read as wood and not as a sine.
const PARTIALS: [(f32, f32, f32); 3] = [(1.0, 1.0, 1.0), (2.0, 0.30, 2.2), (3.0, 0.10, 3.4)];

/// One note, started `start` seconds in: a 7 ms raised-cosine attack, which
/// has no edge to click, then an exponential decay.
fn note(t: f32, hz: f32, start: f32) -> f32 {
    let local = t - start;
    if local < 0.0 {
        return 0.0;
    }
    let attack =
        if local < 0.007 { 0.5 - 0.5 * (std::f32::consts::PI * local / 0.007).cos() } else { 1.0 };
    PARTIALS
        .iter()
        .map(|&(multiple, level, faster)| {
            level * (TAU * hz * multiple * local).sin() * (-local * 11.0 * faster).exp()
        })
        .sum::<f32>()
        * attack
}

fn main() -> anyhow::Result<()> {
    let out =
        std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: make-chime <output.wav>"))?;

    let total = (RATE as f32 * SECONDS) as usize;
    let dry: Vec<f32> = (0..total)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            NOTES.iter().map(|&(hz, start)| note(t, hz, start) * 0.55).sum()
        })
        .collect();

    // Two quiet echoes, the second softer: a little room around the notes.
    let echo = |delay_seconds: f32, gain: f32, n: usize| {
        let delay = (delay_seconds * RATE as f32) as usize;
        if n >= delay { dry[n - delay] * gain } else { 0.0 }
    };
    let mut wet: Vec<f32> =
        (0..total).map(|n| dry[n] + echo(0.105, 0.22, n) + echo(0.21, 0.09, n)).collect();

    // One-pole low-pass at roughly 4.5 kHz: nothing in the sound needs more, and
    // everything above it is the part that reads as harsh.
    let alpha = 1.0 - (-TAU * 4_500.0 / RATE as f32).exp();
    let mut state = 0.0;
    for sample in &mut wet {
        state += alpha * (*sample - state);
        *sample = state;
    }

    // A 25 ms fade at the end so the file stops at exactly zero.
    let fade = (0.025 * RATE as f32) as usize;
    for k in 0..fade {
        wet[total - 1 - k] *= k as f32 / fade as f32;
    }

    let peak = wet.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let scale = 0.45 / peak;
    let mut pcm = Vec::with_capacity(total * 2);
    for sample in &wet {
        pcm.extend_from_slice(&((sample * scale * f32::from(i16::MAX)) as i16).to_le_bytes());
    }

    std::fs::write(&out, clippy_core::sound::wav_from_pcm(&pcm, 1, RATE, 16))?;
    println!("wrote {out} ({} bytes, peak {:.2})", 44 + pcm.len(), 0.45);
    Ok(())
}
