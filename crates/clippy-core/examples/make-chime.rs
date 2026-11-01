//! Generates the chime Clippy plays when it saves a clip.
//!
//! Run once; its output is committed. It exists so the sound in the repo has a
//! recipe rather than a provenance — nobody has to wonder where a binary blob
//! came from or what licence it carries.
//!
//! Usage: cargo run -p clippy-core --example make-chime -- crates/clippy-daemon/assets/clip.wav
//!
//! Two soft, round notes a fifth apart and rising (G4 then D5): a quiet "got
//! it". The previous recipe (C5/G5 plucks with wooden partials and an echo)
//! was judged harsh over game audio. This one is almost pure sine, with a slow
//! 18 ms attack that has no click, a long gentle decay, everything above about
//! 2.2 kHz rolled off, and a peak near -11 dBFS.

use std::f32::consts::{PI, TAU};

const RATE: u32 = 44_100;
const SECONDS: f32 = 0.7;

/// (frequency, start in seconds, level).
const NOTES: [(f32, f32, f32); 2] = [(392.00, 0.0, 0.8), (587.33, 0.09, 1.0)];

/// One note: raised-cosine attack, exponential decay, and a faint octave that
/// fades faster than the fundamental so it reads as glass, not as a test tone.
fn note(t: f32, hz: f32, start: f32) -> f32 {
    let local = t - start;
    if local < 0.0 {
        return 0.0;
    }
    const ATTACK: f32 = 0.018;
    let attack = if local < ATTACK { 0.5 - 0.5 * (PI * local / ATTACK).cos() } else { 1.0 };
    let body = (TAU * hz * local).sin() * (-local * 6.5).exp();
    let octave = 0.10 * (TAU * hz * 2.0 * local).sin() * (-local * 16.0).exp();
    (body + octave) * attack
}

fn main() -> anyhow::Result<()> {
    let out =
        std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: make-chime <output.wav>"))?;

    let total = (RATE as f32 * SECONDS) as usize;
    let mut wet: Vec<f32> = (0..total)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            NOTES.iter().map(|&(hz, start, level)| note(t, hz, start) * level).sum()
        })
        .collect();

    // Two passes of a one-pole low-pass at about 2.2 kHz: nothing above it is
    // needed, and it is the part of a chime that reads as sharp.
    let alpha = 1.0 - (-TAU * 2_200.0 / RATE as f32).exp();
    for _ in 0..2 {
        let mut state = 0.0;
        for sample in &mut wet {
            state += alpha * (*sample - state);
            *sample = state;
        }
    }

    // A 60 ms fade so the file ends at exactly zero.
    let fade = (0.06 * RATE as f32) as usize;
    for k in 0..fade {
        wet[total - 1 - k] *= k as f32 / fade as f32;
    }

    const PEAK: f32 = 0.28;
    let peak = wet.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let mut pcm = Vec::with_capacity(total * 2);
    for sample in &wet {
        pcm.extend_from_slice(&((sample * PEAK / peak * f32::from(i16::MAX)) as i16).to_le_bytes());
    }

    std::fs::write(&out, clippy_core::sound::wav_from_pcm(&pcm, 1, RATE, 16))?;
    println!("wrote {out} ({} bytes, peak {PEAK})", 44 + pcm.len());
    Ok(())
}
