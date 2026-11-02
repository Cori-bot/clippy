//! Surround to stereo, at full level.
//!
//! Asked for stereo from a 7.1 device (gaming headsets with virtual surround
//! show up that way), the Windows audio engine downmixes with coefficients
//! that keep the sum of every channel from clipping: a sound on the front
//! left alone comes out about 13.5 dB quieter (measured on a Logitech PRO X:
//! a -20 dBFS tone captured at -33.5 dBFS). Clips sounded nearly silent and
//! their waveform was flat.
//!
//! So the stream is opened in the device's own channel layout and folded down
//! here: the front pair passes at unity, the other speakers join their side at
//! -3 dB, the LFE is left out (headphones play it through the mains already),
//! and a soft knee keeps the rare all-channels-at-once peak from clipping hard.

/// The speaker each `dwChannelMask` bit stands for, in mask (= stream) order.
const FRONT_LEFT: u32 = 0x1;
const FRONT_RIGHT: u32 = 0x2;
const FRONT_CENTER: u32 = 0x4;
const LOW_FREQUENCY: u32 = 0x8;
const BACK_LEFT: u32 = 0x10;
const BACK_RIGHT: u32 = 0x20;
const FRONT_LEFT_OF_CENTER: u32 = 0x40;
const FRONT_RIGHT_OF_CENTER: u32 = 0x80;
const BACK_CENTER: u32 = 0x100;
const SIDE_LEFT: u32 = 0x200;
const SIDE_RIGHT: u32 = 0x400;

const MINUS_3_DB: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Per-channel (left, right) gains for one device layout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Downmix {
    gains: Vec<(f32, f32)>,
}

impl Downmix {
    /// The fold-down for `channels` interleaved channels laid out as `mask`
    /// says. A mask of 0 (some drivers report none) means the usual layout
    /// for that many channels.
    pub(crate) fn new(channels: usize, mask: u32) -> Self {
        let mask =
            if mask.count_ones() as usize == channels { mask } else { default_mask(channels) };
        let mut speakers = (0..32).map(|bit| 1u32 << bit).filter(|bit| mask & bit != 0);
        let gains = (0..channels)
            .map(|index| match speakers.next() {
                Some(speaker) => speaker_gains(speaker),
                // More channels than the mask names: the first two are the
                // front pair, anything else goes to both sides.
                None if index == 0 => (1.0, 0.0),
                None if index == 1 => (0.0, 1.0),
                None => (MINUS_3_DB, MINUS_3_DB),
            })
            .collect();
        Self { gains }
    }

    pub(crate) fn channels(&self) -> usize {
        self.gains.len()
    }

    /// Folds interleaved f32 frames down to interleaved stereo f32.
    pub(crate) fn fold(&self, input: &[f32], output: &mut Vec<f32>) {
        let channels = self.gains.len();
        output.reserve(input.len() / channels * 2);
        for frame in input.chunks_exact(channels) {
            let (mut left, mut right) = (0.0f32, 0.0f32);
            for (sample, (gain_left, gain_right)) in frame.iter().zip(&self.gains) {
                left += sample * gain_left;
                right += sample * gain_right;
            }
            output.push(soft_clip(left));
            output.push(soft_clip(right));
        }
    }
}

fn speaker_gains(speaker: u32) -> (f32, f32) {
    match speaker {
        FRONT_LEFT => (1.0, 0.0),
        FRONT_RIGHT => (0.0, 1.0),
        FRONT_CENTER | BACK_CENTER => (MINUS_3_DB, MINUS_3_DB),
        LOW_FREQUENCY => (0.0, 0.0),
        BACK_LEFT | SIDE_LEFT | FRONT_LEFT_OF_CENTER => (MINUS_3_DB, 0.0),
        BACK_RIGHT | SIDE_RIGHT | FRONT_RIGHT_OF_CENTER => (0.0, MINUS_3_DB),
        // Top speakers and anything rarer: half to each side.
        _ => (0.5, 0.5),
    }
}

/// Windows' standard layouts (ksmedia.h `KSAUDIO_SPEAKER_*`).
fn default_mask(channels: usize) -> u32 {
    match channels {
        1 => FRONT_CENTER,
        2 => FRONT_LEFT | FRONT_RIGHT,
        3 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER,
        4 => FRONT_LEFT | FRONT_RIGHT | BACK_LEFT | BACK_RIGHT,
        5 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER | BACK_LEFT | BACK_RIGHT,
        6 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER | LOW_FREQUENCY | SIDE_LEFT | SIDE_RIGHT,
        8 => {
            FRONT_LEFT
                | FRONT_RIGHT
                | FRONT_CENTER
                | LOW_FREQUENCY
                | BACK_LEFT
                | BACK_RIGHT
                | SIDE_LEFT
                | SIDE_RIGHT
        }
        _ => 0,
    }
}

/// Unity below the knee, then a smooth approach to full scale: summing
/// channels can exceed 1.0, and a hard clamp would crackle.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let headroom = 1.0 - KNEE;
    (KNEE + headroom * ((magnitude - KNEE) / headroom).tanh()).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURROUND_7_1: u32 = 0x63F;

    fn fold(downmix: &Downmix, frame: &[f32]) -> (f32, f32) {
        let mut out = Vec::new();
        downmix.fold(frame, &mut out);
        (out[0], out[1])
    }

    #[test]
    fn a_front_left_sound_on_a_7_1_device_keeps_its_level() {
        let downmix = Downmix::new(8, SURROUND_7_1);
        // FL FR FC LFE BL BR SL SR
        let (left, right) = fold(&downmix, &[0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((left - 0.1).abs() < 1e-6, "front left must pass at unity, got {left}");
        assert_eq!(right, 0.0);
    }

    #[test]
    fn centre_and_surrounds_join_their_sides_and_the_lfe_is_left_out() {
        let downmix = Downmix::new(8, SURROUND_7_1);
        let (left, right) = fold(&downmix, &[0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(
            (left - 0.5 * MINUS_3_DB).abs() < 1e-6 && (right - left).abs() < 1e-6,
            "centre goes to both at -3 dB"
        );
        let (left, right) = fold(&downmix, &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.4]);
        assert_eq!(left, 0.0);
        assert!((right - 0.4 * MINUS_3_DB).abs() < 1e-6, "side right goes right");
        assert_eq!(fold(&downmix, &[0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.0, 0.0]), (0.0, 0.0), "LFE");
    }

    #[test]
    fn every_channel_at_full_scale_stays_within_full_scale() {
        let downmix = Downmix::new(8, SURROUND_7_1);
        let (left, right) = fold(&downmix, &[1.0; 8]);
        assert!(left <= 1.0 && left > 0.95, "{left}");
        assert!(right <= 1.0 && right > 0.95, "{right}");
        let (left, _) = fold(&downmix, &[-1.0; 8]);
        assert!(left >= -1.0 && left < -0.95);
    }

    #[test]
    fn a_missing_mask_means_the_usual_layout() {
        assert_eq!(Downmix::new(8, 0), Downmix::new(8, SURROUND_7_1));
        assert_eq!(Downmix::new(2, 0).gains, vec![(1.0, 0.0), (0.0, 1.0)]);
    }

    #[test]
    fn whole_frames_only() {
        let downmix = Downmix::new(6, 0);
        let mut out = Vec::new();
        downmix.fold(&[0.1; 6 * 3 + 2], &mut out);
        assert_eq!(out.len(), 6, "three stereo frames; the partial one is dropped");
    }
}
