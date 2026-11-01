//! Generates the sound Clippy plays when it saves a screenshot.
//!
//! Run once; its output is committed. It exists for the same reason
//! `make-chime.rs` does — the sound in the repo has a recipe rather than a
//! provenance nobody can check.
//!
//! Usage: cargo run -p clippy-core --example make-shot-sound -- crates/clippy-daemon/assets/shot.wav
//!
//! One soft "pop": a sine that glides up from 520 to 780 Hz in 45 ms and dies
//! away in about 150 ms. The previous recipe (noise ticks over a thump, a
//! shutter imitation) was judged harsh. A single short note is still told
//! apart from the clip chime's two longer ones, and it has no noise in it at
//! all, which is what made the shutter scratchy.

use std::f32::consts::{PI, TAU};

const RATE: u32 = 44_100;
const SECONDS: f32 = 0.2;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: make-shot-sound <output.wav>"))?;

    let total = (RATE as f32 * SECONDS) as usize;
    let mut phase = 0.0f32;
    let mut wave = Vec::with_capacity(total);
    for n in 0..total {
        let t = n as f32 / RATE as f32;
        // Ease-out glide, integrated as phase so the pitch moves without a
        // discontinuity.
        let glide = (t / 0.045).min(1.0);
        let hz = 520.0 + 260.0 * (1.0 - (1.0 - glide) * (1.0 - glide));
        phase += TAU * hz / RATE as f32;
        const ATTACK: f32 = 0.006;
        let attack = if t < ATTACK { 0.5 - 0.5 * (PI * t / ATTACK).cos() } else { 1.0 };
        wave.push(phase.sin() * attack * (-t * 26.0).exp());
    }

    let alpha = 1.0 - (-TAU * 2_500.0 / RATE as f32).exp();
    let mut state = 0.0;
    for sample in &mut wave {
        state += alpha * (*sample - state);
        *sample = state;
    }
    let fade = (0.03 * RATE as f32) as usize;
    for k in 0..fade {
        wave[total - 1 - k] *= k as f32 / fade as f32;
    }

    const PEAK: f32 = 0.24;
    let peak = wave.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let mut pcm = Vec::with_capacity(total * 2);
    for sample in &wave {
        pcm.extend_from_slice(&((sample * PEAK / peak * f32::from(i16::MAX)) as i16).to_le_bytes());
    }

    std::fs::write(&out, clippy_core::sound::wav_from_pcm(&pcm, 1, RATE, 16))?;
    println!("wrote {out} ({} bytes)", 44 + pcm.len());
    Ok(())
}
