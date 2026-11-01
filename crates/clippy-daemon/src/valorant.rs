//! Valorant kills, read off the kill feed in the captured picture.
//!
//! Riot offers no local event feed for Valorant, and anything reading the
//! game's memory is off limits (Vanguard). What every player sees is enough:
//! the local player's entries in the kill feed (top right) carry a yellow
//! frame around their agent portrait. On the left end of a row, the player
//! is the killer; on the right end, the victim. The local player is always
//! on the ally (teal) side of that row.
//!
//! The detector works on the region [`REGION`] reads back from the live
//! capture ([`clippy_core::capture::probe`]), a few times a second.

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

/// One kill-feed row, told apart from the others by where the local
/// player's frame sits and how long the bar is (both follow the names'
/// lengths), in pixels at the 1080p reference scale. Rows move up as older
/// ones expire; this does not.
pub type RowSig = (u16, u16);

/// What the kill feed shows about the local player.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FeedReading {
    /// Rows where they are the killer.
    pub kills: Vec<RowSig>,
    /// Rows where they are the victim.
    pub deaths: u32,
}

/// Reads the kill feed in a BGRA picture of [`REGION`].
pub fn feed_rows(bgra: &[u8], width: usize, height: usize, stride: usize) -> FeedReading {
    let mut reading = FeedReading::default();
    if width < 8 || height < 8 || stride < width * 4 || bgra.len() < stride * height {
        return reading;
    }
    let s = height as f32 / REFERENCE_HEIGHT;
    let (mut teal, mut red, mut yellow) =
        (vec![false; width * height], vec![false; width * height], vec![false; width * height]);
    for y in 0..height {
        for x in 0..width {
            let p = &bgra[y * stride + x * 4..];
            let c = hsv(p[0], p[1], p[2]);
            let i = y * width + x;
            teal[i] = (72..=92).contains(&c.h) && c.s > 40 && c.v > 90;
            red[i] = (c.h <= 8 || c.h >= 172) && c.s > 100 && c.v > 160;
            yellow[i] = (22..=34).contains(&c.h) && c.s > 110 && c.v > 170;
        }
    }
    let at = |m: &[bool], x: usize, y: usize| m[y * width + x];
    // A yellow line through most of the region is a screen effect (the
    // golden edge of the death screen), never a portrait frame.
    let mut border = vec![false; width];
    for x in 0..width {
        let tall = (0..height).filter(|&y| at(&yellow, x, y)).count() as f32 >= height as f32 * 0.45;
        if tall {
            for b in x.saturating_sub(3)..(x + 4).min(width) {
                border[b] = true;
            }
        }
    }
    let cover: Vec<usize> =
        (0..height).map(|y| (0..width).filter(|&x| at(&teal, x, y) || at(&red, x, y)).count()).collect();
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
        let bar_cols: Vec<usize> = (0..width)
            .filter(|&x| {
                (y0..y1).filter(|&r| at(&teal, x, r) || at(&red, x, r)).count() * 2 >= span
            })
            .collect();
        let (Some(&bx0), Some(&bx1)) = (bar_cols.first(), bar_cols.last()) else { continue };
        let pad = (6.0 * s) as usize;
        let (ys0, ys1) = (y0.saturating_sub(pad), (y1 + pad).min(height));
        let frame_cols: Vec<usize> = (0..width)
            .filter(|&x| {
                !border[x]
                    && (ys0..ys1).filter(|&r| at(&yellow, x, r)).count() as f32 >= span as f32 * 0.6
            })
            .collect();
        let teal_share = |a: usize, b: usize| {
            let (mut t, mut r) = (0usize, 0usize);
            for row in y0..y1 {
                for x in a..b.min(width) {
                    t += usize::from(at(&teal, x, row));
                    r += usize::from(at(&red, x, row));
                }
            }
            t as f32 / (t + r).max(1) as f32
        };
        let mid = (bx0 + bx1) / 2;
        let near = |lo: isize, hi: isize| {
            frame_cols.iter().any(|&x| (x as isize) >= lo && (x as isize) <= hi)
        };
        let (bx0i, bx1i, spani) = (bx0 as isize, bx1 as isize, span as isize);
        let killer = near(bx0i - 2 * spani, bx0i + spani) && teal_share(bx0, mid) > 0.6;
        let victim = near(bx1i - spani, bx1i + 2 * spani) && teal_share(mid, bx1 + 1) > 0.6;
        if victim {
            reading.deaths += 1;
        } else if killer {
            let first = frame_cols.iter().copied().find(|&x| x + 2 * span >= bx0).unwrap_or(bx0);
            let reference = |v: usize| (v as f32 / s).round() as u16;
            reading.kills.push((reference(first), reference(bx1 - bx0)));
        }
    }
    reading
}

/// Turns kill-feed readings into new kills. A row counts once, when it has
/// been seen in two readings in a row (one-sample flicker is not a kill),
/// and is remembered while it stays on screen plus [`FORGET`] readings, so
/// fading behind an effect and coming back does not count it again.
#[derive(Debug, Default)]
pub struct RowTracker {
    sample: u32,
    /// Counted rows and the last reading that showed them.
    known: Vec<(RowSig, u32)>,
    /// Rows seen once, at the given reading, not yet counted.
    candidates: Vec<(RowSig, u32)>,
}

/// Readings a counted row is remembered after it was last seen (5 s at 200 ms).
const FORGET: u32 = 25;
/// Signature distance, in reference pixels, still taken as the same row.
const SAME_ROW: u16 = 4;

fn same(a: RowSig, b: RowSig) -> bool {
    a.0.abs_diff(b.0) <= SAME_ROW && a.1.abs_diff(b.1) <= SAME_ROW
}

impl RowTracker {
    /// New kills in this reading.
    pub fn push(&mut self, rows: &[RowSig]) -> u32 {
        self.sample += 1;
        let now = self.sample;
        self.known.retain(|&(_, seen)| now - seen <= FORGET);
        self.candidates.retain(|&(_, seen)| now - seen <= 1);
        let mut new = 0;
        for &row in rows {
            if let Some(known) = self.known.iter_mut().find(|(sig, _)| same(*sig, row)) {
                known.1 = now;
            } else if let Some(i) = self.candidates.iter().position(|(sig, _)| same(*sig, row)) {
                self.candidates.swap_remove(i);
                self.known.push((row, now));
                new += 1;
            } else {
                self.candidates.push((row, now));
            }
        }
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_matches_opencv_on_the_feed_colours() {
        // Kill-feed teal, red and the local player's yellow frame (BGR).
        let teal = hsv(160, 190, 90);
        assert!((72..=92).contains(&teal.h), "{}", teal.h);
        let red = hsv(90, 90, 235);
        assert!(red.h <= 8 && red.s > 100);
        let yellow = hsv(100, 225, 235);
        assert!((22..=34).contains(&yellow.h) && yellow.s > 110 && yellow.v > 170, "{}", yellow.h);
    }

    /// A 576x270 region with one feed row: a yellow-framed portrait at
    /// `frame_x`, then a bar teal on `teal_left`'s side and red on the other.
    fn picture(frame_x: usize, teal_left: bool) -> Vec<u8> {
        let (w, h) = (576, 270);
        let mut px = vec![40u8; w * h * 4];
        let mut put = |x: usize, y: usize, bgr: [u8; 3]| {
            px[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&bgr);
        };
        let (teal, red, yellow) = ([160, 190, 90], [90, 90, 235], [100, 225, 235]);
        for y in 20..55 {
            for x in 150..555 {
                let left = x < 350;
                put(x, y, if left == teal_left { teal } else { red });
            }
            for x in [frame_x, frame_x + 1, frame_x + 50, frame_x + 51] {
                put(x, y, yellow);
            }
        }
        px
    }

    #[test]
    fn a_yellow_frame_on_the_teal_left_end_is_a_kill() {
        let px = picture(96, true);
        let reading = feed_rows(&px, 576, 270, 576 * 4);
        assert_eq!((reading.kills.len(), reading.deaths), (1, 0));
        assert_eq!(reading.kills[0], (96, 404));
    }

    #[test]
    fn a_yellow_frame_on_the_teal_right_end_is_a_death() {
        let px = picture(520, false);
        assert_eq!(feed_rows(&px, 576, 270, 576 * 4), FeedReading { kills: vec![], deaths: 1 });
    }

    #[test]
    fn a_frame_beside_an_enemy_killer_is_not_a_kill() {
        // Red on the left: an enemy killed someone; the yellow is not ours.
        let px = picture(96, false);
        assert!(feed_rows(&px, 576, 270, 576 * 4).kills.is_empty());
    }

    #[test]
    fn a_row_counts_once_even_when_it_flickers_or_moves_up() {
        let (a, b) = ((96, 404), (240, 300));
        let mut t = RowTracker::default();
        assert_eq!(t.push(&[a]), 0, "one sighting is not enough");
        assert_eq!(t.push(&[a]), 1);
        assert_eq!(t.push(&[]) + t.push(&[(97, 403)]), 0, "flicker, then the same row");
        // A second kill while the first is still listed, and again 7 s later
        // with the first one gone: both count.
        assert_eq!(t.push(&[a, b]) + t.push(&[a, b]), 1);
        for _ in 0..30 {
            t.push(&[]);
        }
        assert_eq!(t.push(&[a]) + t.push(&[a]), 1, "a later kill with the same names");
    }

    #[test]
    fn a_single_sighting_never_counts() {
        let mut t = RowTracker::default();
        assert_eq!(t.push(&[(10, 10)]) + t.push(&[]) + t.push(&[(200, 200)]), 0);
    }
}

/// Opt-in: regions cut from real recordings (`.test-tmp/valo/cases.txt`:
/// `file width height kills deaths` per line), checked against the
/// prototype's reading. `cargo test -p clippy-daemon real_valorant -- --ignored`
#[cfg(test)]
#[test]
#[ignore]
fn real_valorant_feed_regions() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp/valo");
    let cases = std::fs::read_to_string(dir.join("cases.txt")).unwrap();
    for line in cases.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (w, h): (usize, usize) = (f[1].parse().unwrap(), f[2].parse().unwrap());
        let px = std::fs::read(dir.join(f[0])).unwrap();
        let got = feed_rows(&px, w, h, w * 4);
        println!("{} -> {got:?}", f[0]);
        let counts = (got.kills.len(), got.deaths as usize);
        assert_eq!(counts, (f[3].parse().unwrap(), f[4].parse().unwrap()), "{line}");
    }
}
