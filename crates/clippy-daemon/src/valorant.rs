//! Valorant kills, read off the kill feed in the captured picture.
//!
//! Riot offers no local event feed for Valorant, and anything reading the
//! game's memory is off limits (Vanguard). What every player sees is enough:
//! the local player's entries in the kill feed (top right) are outlined in
//! yellow-green: a thin line along the top and bottom of the bar, on the
//! player's side, around their portrait and name. The local player is always
//! on the ally (teal) side of their rows; teal on the left is the killer,
//! teal on the right the victim. Teammates' rows are teal too, but carry no
//! outline. In Deathmatch every other row is red on both sides.
//!
//! The detector works on the region [`REGION`] reads back from the live
//! capture ([`clippy_core::capture::probe`]), a few times a second.
//!
//! Measured on real 1080p footage (ranked and Deathmatch): the outline
//! covers 40–100 % of the player's section on each edge (one edge is
//! sometimes lost to compression or the row below), other rows 0–10 %.
//! A bar's left end is unreliable (the killer portrait, assist icons and
//! the slide-in animation move it by up to 70 px), its right end is fixed
//! (the feed is right-aligned); the teal/red boundary does not move while a
//! row is listed, so it is what tells two rows apart.

use clippy_core::capture::probe::ProbeRegion;

/// The kill feed: the right 30 % of the screen, 7 % to 32 % down.
pub const REGION: ProbeRegion = (700, 70, 300, 250);
/// Process name of the game client.
pub const EXE: &str = "valorant-win64-shipping.exe";
/// Region height at 1080p, the scale the pixel thresholds are tuned at.
const REFERENCE_HEIGHT: f32 = 270.0;

#[derive(Clone, Copy)]
struct Hsv {
    h: u8,
    s: u8,
    v: u8,
}

/// OpenCV's 8-bit HSV: hue 0..180, saturation and value 0..255.
fn hsv(b: u8, g: u8, r: u8) -> Hsv {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let v = max;
    if max == 0 || max == min {
        return Hsv { h: 0, s: 0, v };
    }
    let delta = f32::from(max - min);
    let s = (delta * 255.0 / f32::from(max)).round() as u8;
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let mut h = if max as f32 == r {
        60.0 * (g - b) / delta
    } else if max as f32 == g {
        120.0 + 60.0 * (b - r) / delta
    } else {
        240.0 + 60.0 * (r - g) / delta
    };
    if h < 0.0 {
        h += 360.0;
    }
    Hsv { h: (h / 2.0).round().min(179.0) as u8, s, v }
}

/// Pixel classes, as bits.
const TEAL: u8 = 1;
const RED: u8 = 2;
/// The local player's outline: yellow on the portrait, fading to green
/// where it runs along the teal part.
const LINE: u8 = 4;

fn class(c: Hsv) -> u8 {
    let mut k = 0;
    if (72..=92).contains(&c.h) && c.s > 40 && c.v > 90 {
        k |= TEAL;
    }
    if (c.h <= 8 || c.h >= 172) && c.s > 100 && c.v > 160 {
        k |= RED;
    }
    if (30..=72).contains(&c.h) && c.s >= 60 && c.v >= 180 {
        k |= LINE;
    }
    k
}

/// One of the local player's kill-feed rows: its top edge and where its
/// teal part meets the red one, in pixels at the 1080p reference scale.
/// Rows move up as older ones expire; the boundary does not move.
pub type RowSig = (u16, u16);

/// What the kill feed shows about the local player.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FeedReading {
    /// Rows where they are the killer, top to bottom.
    pub kills: Vec<RowSig>,
    /// Rows where they are the victim.
    pub deaths: u32,
    /// Feed rows seen, anyone's.
    pub rows: u32,
}

/// Share of the player's section the outline must cover, top and bottom
/// edges added: 0.6 is met by one clean edge alone, or two partial ones.
const OUTLINE: f32 = 0.6;

/// Reads the kill feed in a BGRA picture of [`REGION`].
pub fn feed_rows(bgra: &[u8], width: usize, height: usize, stride: usize) -> FeedReading {
    let mut reading = FeedReading::default();
    if width < 8 || height < 8 || stride < width * 4 || bgra.len() < stride * height {
        return reading;
    }
    let s = height as f32 / REFERENCE_HEIGHT;
    let mut px = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let p = &bgra[y * stride + x * 4..];
            px[y * width + x] = class(hsv(p[0], p[1], p[2]));
        }
    }
    let at = |x: usize, y: usize, k: u8| px[y * width + x] & k != 0;
    let cover: Vec<usize> =
        (0..height).map(|y| (0..width).filter(|&x| at(x, y, TEAL | RED)).count()).collect();
    let reference = |v: usize| (v as f32 / s).round() as u16;
    let band = ((3.0 * s).round() as usize).max(1);
    let mut y = 0;
    while y < height {
        if (cover[y] as f32) < 60.0 * s {
            y += 1;
            continue;
        }
        let y0 = y;
        while y < height && cover[y] as f32 >= 60.0 * s {
            y += 1;
        }
        let (y1, span) = (y, y - y0);
        if (span as f32) < 18.0 * s {
            continue;
        }
        let count = |x: usize, k: u8| (y0..y1).filter(|&r| at(x, r, k)).count();
        let (tc, rc): (Vec<usize>, Vec<usize>) =
            (0..width).map(|x| (count(x, TEAL), count(x, RED))).unzip();
        let bar = |x: usize| (tc[x] + rc[x]) * 2 >= span;
        let (Some(bx0), Some(bx1)) =
            ((0..width).find(|&x| bar(x)), (0..width).rev().find(|&x| bar(x)))
        else {
            continue;
        };
        reading.rows += 1;
        let first_teal = (bx0..=bx1).find(|&x| bar(x) && tc[x] > rc[x]);
        let first_red = (bx0..=bx1).find(|&x| bar(x) && rc[x] > tc[x]);
        let (Some(teal), Some(red)) = (first_teal, first_red) else { continue };
        let victim_width = (1.5 * span as f32) as usize;
        // The player's section of the row, and where it meets the other.
        let (a, b, split, kill) = if teal < red {
            // Killer side. Rows still sliding in show little of the victim.
            if bx1 + 1 - red < victim_width {
                continue;
            }
            let (t, r): (usize, usize) =
                (bx0..red).map(|x| (tc[x], rc[x])).fold((0, 0), |(t, r), (a, b)| (t + a, r + b));
            if (t as f32) <= 0.6 * (t + r) as f32 {
                continue;
            }
            (bx0, red, red, true)
        } else {
            if teal - bx0 < victim_width {
                continue;
            }
            // The outline also runs around the victim's portrait, past the bar.
            (teal, (bx1 + 1 + (1.4 * span as f32) as usize).min(width), teal, false)
        };
        let edge = |from: usize, to: usize| {
            let lined = (a..b).filter(|&x| (from..to.min(height)).any(|r| at(x, r, LINE))).count();
            lined as f32 / (b - a).max(1) as f32
        };
        let top = edge(y0.saturating_sub(band), y0 + band);
        let bottom = edge(y1.saturating_sub(band), y1 + band);
        if top + bottom < OUTLINE {
            continue;
        }
        if kill {
            reading.kills.push((reference(y0), reference(split)));
        } else {
            reading.deaths += 1;
        }
    }
    reading
}

/// Turns kill-feed readings into new kills.
///
/// Rows are followed by order and by their teal/red boundary: rows only
/// ever move up, older ones expire at the top and new ones come in at the
/// bottom. Each reading is matched, in top-to-bottom order, against the
/// remembered rows with the same boundary that it is not below. When a
/// sighting could be either of two remembered rows (the local player's rows
/// often share a boundary: same name, same victim name length), the one
/// left over is the one that can have expired: the top one once it has been
/// listed [`EXPIRE`] readings, otherwise the lower one (missed for a moment).
///
/// A row that matches nothing is a candidate; seen again at the next reading
/// (a single sighting is flicker or the slide-in), it is a kill. A row
/// missed for a moment (an effect over the feed, the outline lost to
/// compression) is remembered for [`FORGET`] readings, but only until it is
/// [`LIFE`] readings old: past that it has left the feed, and a row with the
/// same boundary in its place is a new kill.
#[derive(Debug, Default)]
pub struct RowTracker {
    sample: u32,
    /// Counted rows.
    known: Vec<Known>,
    /// Rows seen once, at the previous reading, not yet counted.
    candidates: Vec<RowSig>,
}

#[derive(Debug, Clone, Copy)]
struct Known {
    row: RowSig,
    /// Readings that first and last showed it.
    first: u32,
    seen: u32,
}

/// Readings a counted row is remembered after it was last seen (3 s at 200 ms).
const FORGET: u32 = 15;
/// Readings a row stays listed at least, unless a full feed pushes it out.
/// Measured on real footage: 19 to 25 (4 to 5 s).
const EXPIRE: u32 = 18;
/// Readings after which a row has left the feed (5 s; the longest seen
/// was last read at 24).
const LIFE: u32 = 25;
/// Boundary distance, in reference pixels, still taken as the same row.
const SAME_X: u16 = 10;
/// How far down, in reference pixels, a row may read and still be the same
/// (rows never move down; this is jitter).
const DOWN: u16 = 10;
/// How far up a candidate may have moved between its two sightings (a row
/// is 39 px tall).
const UP: u16 = 45;

impl RowTracker {
    /// New kills in this reading.
    pub fn push(&mut self, rows: &[RowSig]) -> u32 {
        self.sample += 1;
        let now = self.sample;
        // Missed at the last reading and too old to still be listed: gone.
        self.known
            .retain(|k| now - k.seen <= FORGET && (k.seen + 1 >= now || now - k.first <= LIFE));
        self.known.sort_by_key(|k| k.row.0);
        let mut rows = rows.to_vec();
        rows.sort_by_key(|&(y, _)| y);
        let matched = self.assign(&rows, now);
        let mut new = 0;
        let mut candidates = Vec::new();
        for (i, &(y, x)) in rows.iter().enumerate() {
            if let Some(k) = matched[i] {
                self.known[k].row = (y, x);
                self.known[k].seen = now;
                continue;
            }
            let seen = self
                .candidates
                .iter()
                .position(|&(cy, cx)| cx.abs_diff(x) <= SAME_X && y <= cy + DOWN && y + UP >= cy);
            match seen {
                Some(c) => {
                    self.candidates.swap_remove(c);
                    self.known.push(Known { row: (y, x), first: now, seen: now });
                    new += 1;
                }
                None => candidates.push((y, x)),
            }
        }
        self.candidates = candidates;
        new
    }

    /// The remembered row (index in `known`) each sighting is, keeping the
    /// top-to-bottom order: as many matches as possible, then as few rows
    /// left over that cannot have expired yet, then the least movement.
    fn assign(&self, rows: &[RowSig], now: u32) -> Vec<Option<usize>> {
        let (m, n) = (rows.len(), self.known.len());
        let fits = |i: usize, j: usize| {
            let ((y, x), (ky, kx)) = (rows[i], self.known[j].row);
            kx.abs_diff(x) <= SAME_X && y <= ky + DOWN
        };
        // Leaving a row out that cannot have expired costs more than any
        // movement; any extra match is worth more than both.
        let left_out = |j: usize| if now - self.known[j].first < EXPIRE { 10_000 } else { 0 };
        let moved = |i: usize, j: usize| i64::from(self.known[j].row.0.saturating_sub(rows[i].0));
        // best[i][j]: score of sightings i.. against remembered rows j..
        let mut best = vec![vec![0i64; n + 1]; m + 1];
        for j in (0..n).rev() {
            best[m][j] = best[m][j + 1] - left_out(j);
        }
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                let mut score = (best[i + 1][j]).max(best[i][j + 1] - left_out(j));
                if fits(i, j) {
                    score = score.max(1_000_000 - moved(i, j) + best[i + 1][j + 1]);
                }
                best[i][j] = score;
            }
        }
        let mut matched = vec![None; m];
        let (mut i, mut j) = (0, 0);
        while i < m && j < n {
            if fits(i, j) && best[i][j] == 1_000_000 - moved(i, j) + best[i + 1][j + 1] {
                matched[i] = Some(j);
                (i, j) = (i + 1, j + 1);
            } else if best[i][j] == best[i][j + 1] - left_out(j) {
                j += 1;
            } else {
                i += 1;
            }
        }
        matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_matches_opencv_on_the_feed_colours() {
        // Kill-feed teal, red and the local player's outline (BGR).
        let teal = hsv(160, 190, 90);
        assert!((72..=92).contains(&teal.h), "{}", teal.h);
        let red = hsv(90, 90, 235);
        assert!(red.h <= 8 && red.s > 100);
        // Measured: yellow on the portrait, green along the teal part.
        for (b, g, r) in [(100, 230, 200), (140, 230, 190)] {
            assert_eq!(class(hsv(b, g, r)), LINE, "{b},{g},{r}");
        }
    }

    const W: usize = 576;
    const H: usize = 270;
    const TEAL_PX: [u8; 3] = [160, 190, 90];
    const RED_PX: [u8; 3] = [90, 90, 235];
    const LINE_PX: [u8; 3] = [100, 230, 200];

    struct Picture(Vec<u8>);

    impl Picture {
        fn new() -> Self {
            Self(vec![40u8; W * H * 4])
        }

        fn put(&mut self, x: usize, y: usize, bgr: [u8; 3]) {
            self.0[(y * W + x) * 4..(y * W + x) * 4 + 3].copy_from_slice(&bgr);
        }

        /// A feed row from x 150 to 555 at `top`, 34 px tall, split at
        /// `split`, teal on the left if `teal_left`, outlined on the teal
        /// side if `outlined`.
        fn row(&mut self, top: usize, split: usize, teal_left: bool, outlined: bool) -> &mut Self {
            for y in top..top + 34 {
                for x in 150..556 {
                    let left = x < split;
                    self.put(x, y, if left == teal_left { TEAL_PX } else { RED_PX });
                }
            }
            if outlined {
                let (a, b) = if teal_left { (110, split) } else { (split, 556) };
                for x in a..b {
                    for y in [top, top + 1, top + 32, top + 33] {
                        self.put(x, y, LINE_PX);
                    }
                }
            }
            self
        }

        fn read(&self) -> FeedReading {
            feed_rows(&self.0, W, H, W * 4)
        }
    }

    #[test]
    fn an_outlined_row_teal_on_the_left_is_a_kill() {
        let reading = Picture::new().row(20, 350, true, true).read();
        assert_eq!(reading, FeedReading { kills: vec![(20, 350)], deaths: 0, rows: 1 });
    }

    #[test]
    fn an_outlined_row_teal_on_the_right_is_a_death() {
        let reading = Picture::new().row(20, 350, false, true).read();
        assert_eq!(reading, FeedReading { kills: vec![], deaths: 1, rows: 1 });
    }

    #[test]
    fn a_teammates_row_has_no_outline_and_is_not_a_kill() {
        let reading = Picture::new().row(20, 350, true, false).row(60, 380, false, false).read();
        assert_eq!(reading, FeedReading { kills: vec![], deaths: 0, rows: 2 });
    }

    #[test]
    fn one_edge_of_the_outline_is_enough() {
        let mut p = Picture::new();
        p.row(60, 350, true, true);
        for x in 110..350 {
            for y in [92, 93] {
                p.put(x, y, TEAL_PX);
            }
        }
        assert_eq!(p.read().kills, vec![(60, 350)]);
    }

    #[test]
    fn a_row_still_sliding_in_is_not_read_yet() {
        // The victim's red part not shown yet.
        assert!(Picture::new().row(20, 540, true, true).read().kills.is_empty());
    }

    #[test]
    fn a_row_counts_once_while_it_moves_up_and_flickers() {
        let mut t = RowTracker::default();
        assert_eq!(t.push(&[(177, 402)]), 0, "one sighting is not enough");
        assert_eq!(t.push(&[(177, 402)]), 1);
        // Shifting up as older rows expire, missed now and then.
        let path = [138, 99, 99, 0, 60, 0, 21, 21];
        let counted: u32 =
            path.iter().map(|&y| if y == 0 { t.push(&[]) } else { t.push(&[(y, 402)]) }).sum();
        assert_eq!(counted, 0);
    }

    #[test]
    fn a_second_kill_below_counts_even_with_the_same_boundary() {
        let mut t = RowTracker::default();
        t.push(&[(60, 402)]);
        assert_eq!(t.push(&[(60, 402)]), 1);
        // Same name lengths, new row at the bottom.
        assert_eq!(t.push(&[(21, 402), (177, 402)]), 0);
        assert_eq!(t.push(&[(21, 402), (177, 402)]), 1);
        assert_eq!(t.push(&[(21, 402), (138, 402)]), 0, "both move up");
    }

    #[test]
    fn a_row_gone_for_good_is_forgotten_and_the_next_one_counts() {
        let mut t = RowTracker::default();
        t.push(&[(21, 402)]);
        t.push(&[(21, 402)]);
        for _ in 0..=FORGET {
            t.push(&[]);
        }
        assert_eq!(t.push(&[(21, 402)]) + t.push(&[(21, 402)]), 1);
    }

    /// Feeds `rows` `times` times, returning the kills counted.
    fn feed(t: &mut RowTracker, rows: &[RowSig], times: usize) -> u32 {
        (0..times).map(|_| t.push(rows)).sum()
    }

    #[test]
    fn a_new_kill_in_place_of_an_expired_one_with_the_same_boundary_counts() {
        // Same name, same victim name length: same boundary, same slot.
        let mut t = RowTracker::default();
        assert_eq!(feed(&mut t, &[(21, 402)], 22), 1);
        // It expires, the feed is empty for a second, the next kill comes in.
        assert_eq!(feed(&mut t, &[], 5), 0);
        assert_eq!(feed(&mut t, &[(21, 402)], 10), 1);
    }

    #[test]
    fn a_young_row_missed_for_a_moment_is_not_counted_again() {
        let mut t = RowTracker::default();
        assert_eq!(feed(&mut t, &[(21, 402)], 8), 1);
        assert_eq!(feed(&mut t, &[], 3) + feed(&mut t, &[(21, 402)], 8), 0);
    }

    #[test]
    fn two_rows_with_the_same_boundary_are_told_apart() {
        let mut t = RowTracker::default();
        let (a, b) = ((21, 438), (60, 438));
        assert_eq!(feed(&mut t, &[a], 10) + feed(&mut t, &[a, b], 5), 2);
        // The lower one missed for a moment while the upper one is young.
        assert_eq!(t.push(&[a]) + feed(&mut t, &[a, b], 4), 0);
        // The upper one expires, the lower one moves into its place.
        assert_eq!(feed(&mut t, &[a], 6), 0);
        // And misses a reading.
        assert_eq!(t.push(&[]) + feed(&mut t, &[a], 3), 0);
    }

    #[test]
    fn single_sightings_never_count() {
        let mut t = RowTracker::default();
        assert_eq!(t.push(&[(10, 100)]) + t.push(&[]) + t.push(&[(200, 300)]) + t.push(&[]), 0);
        // A sighting that jumped to another boundary is another row.
        assert_eq!(t.push(&[(21, 540)]) + t.push(&[(21, 438)]), 0);
    }

    /// Frames cut from real recordings (1080p, the [`REGION`] crop), in
    /// `tests/fixtures/valorant`: ranked (teammates' rows teal) and
    /// Deathmatch (every other row red).
    #[test]
    fn real_kill_feed_frames() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valorant");
        // (file, own kills, own deaths)
        let cases = [
            ("dm_two_kills", 2, 0),
            ("dm_kill_one_edge", 2, 0),
            ("dm_kill_and_death", 1, 1),
            ("dm_others_only", 0, 0),
            ("dm_slide_in", 0, 1),
            ("dm_teal_scene", 0, 0),
            ("ranked_two_kills_one_death", 2, 1),
            ("ranked_kill", 1, 0),
            ("ranked_kill_after_teammate", 1, 0),
            ("ranked_teammates", 0, 0),
            ("ranked_enemies", 0, 0),
        ];
        for (name, kills, deaths) in cases {
            let (px, w, h, stride) =
                clippy_core::thumb::decode_jpeg_bgra(&dir.join(format!("{name}.png"))).unwrap();
            let got = feed_rows(&px, w as usize, h as usize, stride);
            assert_eq!((got.kills.len(), got.deaths), (kills, deaths), "{name}: {got:?}");
        }
    }
}

/// Opt-in replay of whole recordings: every `.npy` in `.test-tmp/valo2`
/// (frames of the [`REGION`] crop, BGR, sampled at 5 Hz; written by
/// `tests/fixtures/valorant/extract.py` from the clips) through the
/// detector and the tracker, as the daemon runs them, then through the
/// burst rule. Prints the kills of each clip.
/// `cargo test -p clippy-daemon real_valorant -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn real_valorant_recordings() {
    use crate::autoclip::{Debouncer, Moment};
    use std::time::{Duration, Instant};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp/valo2");
    // Own kills checked by eye on every frame of each clip.
    let expected = [
        ("20261101_221840", 2),
        ("20261101_221849", 3),
        ("20261101_221856", 4),
        ("20261101_221921", 6),
        ("20261101_222019", 3),
        ("20261101_222045", 5),
        ("20261101_222058", 7),
        ("20261101_222152", 6),
    ];
    for (name, want) in expected {
        let bytes = std::fs::read(dir.join(format!("{name}.npy"))).unwrap();
        // .npy v1: magic, version, u16 header length, header, raw data.
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let header = std::str::from_utf8(&bytes[10..10 + header_len]).unwrap();
        let shape: Vec<usize> = header
            .split("'shape': (")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .unwrap()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        let (n, h, w) = (shape[0], shape[1], shape[2]);
        let data = &bytes[10 + header_len..];
        let mut tracker = RowTracker::default();
        let mut debouncer = Debouncer::new(Duration::from_secs(3), Duration::from_secs(55));
        let t0 = Instant::now();
        let (mut frames, mut bursts) = (Vec::new(), Vec::new());
        for i in 0..n {
            let bgr = &data[i * h * w * 3..(i + 1) * h * w * 3];
            let bgra: Vec<u8> = bgr.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
            let reading = feed_rows(&bgra, w, h, w * 4);
            let now = t0 + Duration::from_millis(200 * i as u64);
            let new = tracker.push(&reading.kills);
            if new > 0 {
                frames.push(i);
                debouncer.push(Moment { game: "Valorant", kills: new, feat: None }, now);
            }
            if let Some(burst) = debouncer.due(now) {
                bursts.push(burst.kills);
            }
        }
        if let Some(burst) = debouncer.due(t0 + Duration::from_secs(3600)) {
            bursts.push(burst.kills);
        }
        println!("{name}: {} kills at frames {frames:?}, bursts {bursts:?}", frames.len());
        assert_eq!(frames.len(), want, "{name}");
        assert!(bursts.iter().all(|&k| k < 5), "{name}: {bursts:?}");
    }
}
