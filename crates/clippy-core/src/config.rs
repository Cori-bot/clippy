use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::capture::audio::MAX_VOLUME_PERCENT;

/// Encoder rate-control strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Peak-constrained VBR: hit the target on average, allow bursts up to the
    /// max cap on busy motion. The cap keeps the replay ring's RAM bounded.
    PeakVbr,
    /// Constant bitrate: the most predictable file size, at some quality cost.
    Cbr,
}

/// Which API frames are taken off the screen with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMethod {
    /// Let Clippy choose. In this release that is always [`Self::Wgc`] --
    /// Desktop Duplication earns its way to being a default by holding up in
    /// real use first, and a setting that silently changed how everyone's
    /// clips are recorded is the one thing this key must not be.
    Auto,
    /// Windows Graphics Capture: the shipped behaviour. Composites the cursor,
    /// routes hybrid-GPU output for us, and can rate-limit delivery.
    Wgc,
    /// DXGI Desktop Duplication: not subject to the capture border, and has no
    /// cursor.
    Duplication,
}

/// What the replay records, mirroring the choice NVIDIA's overlay offers: games
/// only, or games and the desktop too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Whatever is on the chosen screen, for as long as the replay is active.
    /// The behaviour Clippy has always had.
    Desktop,
    /// Only while a game is in front. Between games nothing is captured, so
    /// the browser, the video player and the desktop never enter a clip --
    /// which is why this is what NVIDIA does by default and what a recorder
    /// left running all day should do.
    Game,
}

/// How the daemon listens for the clip and screenshot combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMode {
    /// `RegisterHotKey`: ask Windows to reserve the combination. The shipped
    /// behaviour, and the default.
    ///
    /// Reserving it is also what makes it fail: the call is refused outright
    /// when another program already holds the combination, and Clippy then never
    /// sees that key again until it is rebound.
    Standard,
    /// `WH_KEYBOARD_LL`: watch the keyboard instead of reserving anything.
    ///
    /// Sees the key even when another program owns it, because there is
    /// nothing to own — and passes every event on untouched, so whoever else
    /// is listening keeps working too. Opt-in, because a program that watches
    /// the keyboard is a shape some anti-cheat software is wary of, and that
    /// risk lands on the user rather than on us.
    LowLevel,
}

/// The game a replay session is recording, as the engine needs to know it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameContext {
    /// Display name, stored in each clip's metadata.
    pub name: String,
    /// The sub-folder its clips go in; already a valid single path component.
    pub folder: String,
}

/// Per-session overrides of the saved configuration. See [`Config::session`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionContext {
    /// The audio tracks to record instead of the defaults from
    /// [`Config::track_specs`]; a game's profile sets this.
    pub tracks: Option<Vec<crate::capture::audio::TrackSpec>>,
    pub game: Option<GameContext>,
}

/// Engine configuration, loaded from `%APPDATA%\clippy\config.toml`.
/// Missing file or missing keys fall back to defaults; a malformed file is
/// reported and ignored rather than aborting the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Target capture/encode frame rate, 30..=120 (any integer).
    pub fps: u32,
    /// Output resolution, independent of `fps`: `"native"` (the monitor's own
    /// size) or a preset height, `"480p"`, `"720p"`, `"1080p"`, `"1440p"`. The
    /// preset is the short side; the long side follows the monitor's aspect
    /// ratio, so nothing is stretched. See [`encode_size`].
    pub resolution: String,
    /// Quality level: `"low"`, `"medium"`, `"high"`, `"very_high"` or
    /// `"custom"`. A level follows the output resolution and fps (see
    /// [`VideoQuality::reference_kbps`]); `"custom"` uses `bitrate_kbps`.
    ///
    /// Empty in a file written before this key existed: the level is then
    /// read back out of `bitrate_kbps` ([`Config::video_quality`]), so an old
    /// fixed tier becomes the level of the same name.
    #[serde(default)]
    pub video_quality: String,
    /// Target (average) video bitrate in kbit/s, used when `video_quality` is
    /// `"custom"`. `0` = auto: derived from the output resolution and fps by
    /// [`auto_bitrate_kbps`].
    pub bitrate_kbps: u32,
    /// VBR peak-bitrate cap in kbit/s. `0` = auto (1.5× the target). Ignored
    /// in CBR mode. This cap is also the replay ring's worst-case RAM bound.
    pub max_bitrate_kbps: u32,
    /// Rate control: `"vbr"` (peak-constrained, quality-leaning) or `"cbr"`
    /// (predictable size). Unknown values fall back to `"vbr"`.
    pub rate_control: String,
    /// Replay-buffer length in seconds (RAM cost scales with this × bitrate).
    pub replay_seconds: u32,
    /// Zero-based index of the monitor to capture.
    pub monitor_index: u32,
    /// Clip hotkey for `clippy replay`: `mods+key`, e.g. "alt+f10" or
    /// "ctrl+shift+c". Modifiers: ctrl, alt, shift, win; keys: a-z, 0-9,
    /// f1-f24. Rebind when another overlay owns the default.
    pub clip_hotkey: String,
    /// GPU scheduling priority for Clippy's own GPU work: `"low"` (below the
    /// game's — capture drops frames under contention instead of costing game
    /// fps) or `"normal"` (equal footing — smoother capture on a saturated
    /// GPU, at some game-fps cost). Unknown values fall back to `"low"`.
    pub gpu_priority: String,
    /// How frames are taken off the screen: `"auto"` (the default), `"wgc"`
    /// (Windows Graphics Capture) or `"dd"` (DXGI Desktop Duplication).
    ///
    /// Exists because Windows can paint its yellow capture border over a game
    /// while WGC is running and Clippy cannot suppress it — the property is a
    /// property of the *display*, and a `true` from any other app beats Clippy's
    /// `false`. Desktop Duplication is not subject to it. It also cannot
    /// capture the mouse cursor, which is why the Settings row says so.
    ///
    /// `"auto"` is Windows Graphics Capture, always. Desktop Duplication is
    /// opt-in until it has held up in real use — nobody's recording changes
    /// unless they choose it. Unknown values fall back to `"auto"`.
    pub capture_method: String,
    /// Seconds between performance self-reports (working set, CPU-vs-GPU
    /// memory split, per-frame latency). 0 (the default) disables the
    /// periodic line; a final summary is always logged at shutdown.
    pub stats_seconds: u32,
    /// Directory clips are written to. Empty (the default) resolves to
    /// `%USERPROFILE%\Videos\Clippy`.
    pub clip_dir: String,
    /// Disk ceiling for the clip library, in GB (spec §5.4). When the library
    /// exceeds it the daemon deletes the oldest clips that are **not** marked
    /// `favorite`. `0` disables the ceiling entirely, and is the default.
    ///
    /// Opt-in, because the two failure modes are not symmetric. A drive
    /// filling up is visible, gradual, and the user's own to act on; a clip
    /// deleted to stay under a ceiling nobody chose is silent and permanent,
    /// and a clip is the thing this product exists to keep. Clip recorders
    /// are notorious for quietly eating a drive, so the mechanism is here and
    /// one number in Settings arms it -- Clippy just will not delete anything it
    /// was not asked to.
    pub max_library_gb: u32,
    /// How loud the PC's own sound is in saved clips, 0–200.
    ///
    /// `100` is unity and the default: the audio exactly as the system mixed
    /// it. Below that the scale is a squared fader taper, so `50` is roughly
    /// half the perceived loudness rather than half the amplitude. Above it the
    /// scale is linear, so `200` is twice the amplitude — about +6 dB — and can
    /// clip. See `MAX_VOLUME_PERCENT` for why boosting is offered at all.
    ///
    /// `0` does not mute the stream — it never opens it.
    pub system_volume: u32,
    /// How loud the microphone is in saved clips, 0–200, on the same scale as
    /// [`Config::system_volume`].
    ///
    /// `0` leaves the microphone closed rather than captured and multiplied by
    /// zero, so Windows' own microphone-in-use indicator stays off. A recorder
    /// holding the microphone open while its own level reads 0 is
    /// indistinguishable, from outside, from one that is lying about it.
    pub mic_volume: u32,
    /// Executable names (`"spotify.exe"`) whose sound is the only PC sound in
    /// clips. Empty, the default, records everything the PC plays.
    ///
    /// Matching is by file name, lower-cased, not by process id: ids change on
    /// every launch, and a browser or a music player is many processes. For
    /// each name Clippy captures the topmost matching process together with
    /// everything it started (Windows "process loopback"), and notices when
    /// the application starts or quits while the replay is active. Levels
    /// still apply through [`Config::system_volume`]. See [`normalize_audio_apps`].
    pub audio_apps: Vec<String>,
    /// What the replay records: `"desktop"` (everything on screen, the default)
    /// or `"game"` (only while a game is in front, then it stops by itself).
    /// See [`CaptureMode`]. Takes effect immediately; an unknown value falls
    /// back to `"desktop"`.
    pub capture_mode: String,
    /// Executable names always treated as games in `"game"` mode, even when
    /// their window is not full screen. Same spelling rules as
    /// [`Config::audio_apps`].
    pub game_apps: Vec<String>,
    /// File clips in a sub-folder per game (`Bureau` for the desktop), the way
    /// NVIDIA's overlay does, instead of all in the clips folder. Read when the
    /// replay starts.
    pub group_by_game: bool,
    /// What the daemon decided for the session about to start (the game in
    /// front and its audio profile). Never read from or written to
    /// `config.toml` and never sent to clients: it is built by the daemon on a
    /// clone of the config at arm time, the same seam `monitor_index` already
    /// uses.
    #[serde(skip)]
    pub session: SessionContext,
    /// Start the daemon at login (spec §7.3). Opt-in and off by default —
    /// adding yourself to startup uninvited is the behaviour people resent most
    /// in this category.
    ///
    /// **The registry is the source of truth, not this field.** A user who
    /// deletes the `HKCU\…\Run` entry by hand has disabled autostart whatever
    /// this file says, so `config.get` reports the registry and `config.set`
    /// writes it. The key exists here so it round-trips and so a third-party UI
    /// can offer the toggle.
    pub autostart: bool,
    /// Arm capture as soon as the daemon starts, instead of waiting to be asked
    /// (spec §7.3).
    ///
    /// Off by default. Arming holds a hardware encoder and a replay ring's
    /// worth of RAM for as long as it lasts, which is not a cost to take on
    /// somebody's behalf — but for anyone running Clippy with
    /// [`Config::autostart`] it is the difference between a recorder that is
    /// ready and one that was never switched on. Forgetting to arm is the way
    /// people lose the clip they wanted, and they only find out afterwards.
    ///
    /// Read once, at daemon startup. Changing it does not arm or disarm
    /// anything now — it is not in `REQUIRES_REARM` either, because it has
    /// nothing to say about how a session is built.
    pub auto_arm: bool,
    /// Whether the desktop app asks GitHub for a newer release on launch.
    ///
    /// Stored here so the settings page reaches it through the same
    /// `config.get`/`config.set` plumbing as everything else. The daemon never
    /// reads it -- `clippy-ui` is the only consumer. It is deliberately not in
    /// `REQUIRES_REARM`: it has nothing to do with the capture session.
    /// Off by default in this fork: the release repo is a placeholder until
    /// Clippy has a real one.
    #[serde(default)]
    pub check_for_updates: bool,
    /// Whether in-game moments (kills, aces) save a clip by themselves, from
    /// the games' official local feeds (CS2 game state integration, League
    /// of Legends live client data). Off until the user turns it on: it
    /// writes a config file into the CS2 install.
    pub auto_clip: bool,
    /// Kills a burst needs before it is saved (a named feat always is).
    #[serde(default = "default_auto_clip_min_kills")]
    pub auto_clip_min_kills: u32,
    /// Whether the daemon plays a sound when it saves a clip.
    ///
    /// Defaulted `true` for the same reason as `check_for_updates`: a config
    /// file written before this key existed must not read as "off".
    #[serde(default = "default_true")]
    pub clip_sound: bool,
    /// The sound file the user chose, or empty for Clippy's built-in chime.
    ///
    /// This is the **original** — the mp3 they picked — and the daemon never
    /// plays it directly. `PlaySound` decodes only WAV, so the file is
    /// converted once when it is chosen and the converted copy is what plays.
    /// This value exists to be shown in settings and to be reconverted from.
    pub clip_sound_path: String,
    /// Whether the daemon publishes a Discord Rich Presence while it runs.
    ///
    /// On whenever the daemon is up rather than only while armed: the card
    /// says Clippy is what the user clips with, and that is true from the moment
    /// Clippy is running.
    ///
    /// Defaulted `true`, unlike `max_library_gb`, and the asymmetry is
    /// deliberate. A presence is visible the instant it happens — it is a card
    /// on the user's own profile — reversible from Settings in two clicks, and
    /// it costs nothing if they never notice it. The library ceiling was
    /// opt-in because *its* mistake was silent and permanent.
    ///
    /// Not telemetry, and it does not become telemetry by being on: presence is
    /// local IPC to the user's own Discord client (see
    /// `clippy-daemon/src/presence.rs`), so nothing about them leaves the
    /// machine, and with Discord shut it does nothing at all.
    #[serde(default = "default_true")]
    pub discord_presence: bool,
    /// Hotkey for a screenshot, same grammar as [`Self::clip_hotkey`].
    ///
    /// `alt+f8` is chosen to be free rather than to be memorable. NVIDIA's
    /// overlay owns `alt+f9` (record), `alt+f10` (save replay) and `alt+f1`
    /// (screenshot), and Steam owns `f12` — a default landing on one of those
    /// registers as a failure the user currently has no way to see.
    #[serde(default = "default_screenshot_hotkey")]
    pub screenshot_hotkey: String,
    /// Whether a saved screenshot plays its chime.
    ///
    /// Its own sound rather than the clip chime, so the two are distinguishable
    /// by ear mid-game — the only moment either sound matters. There is no
    /// custom-file setting to match `clip_sound_path`: a second picker, cache
    /// and repair path is a great deal of surface for a sound that plays a few
    /// times a session.
    #[serde(default = "default_true")]
    pub screenshot_sound: bool,
    /// Whether a saved clip or screenshot shows the on-screen banner (top
    /// right, over the game, never recorded).
    #[serde(default = "default_true")]
    pub clip_banner: bool,
    /// Where the banner shows: `"top_right"` (default) or `"top_left"`.
    /// Unknown values fall back to top right.
    #[serde(default = "default_banner_position")]
    pub banner_position: String,
    /// Volume of the clip and screenshot sounds, 0..=100 (100 = as recorded).
    #[serde(default = "default_notification_volume")]
    pub notification_volume: u32,
    /// Detailed diagnostics: a debug-level daemon log, a log of what is done
    /// in the window, and app + PC metrics every few seconds, all under
    /// `%APPDATA%\clippy\logs`. Off by default; meant for testing builds.
    #[serde(default)]
    pub diagnostics: bool,
    /// How both hotkeys are listened for: `"standard"` (the default) or
    /// `"low_level"`. Unknown values fall back to `"standard"`.
    ///
    /// One key for both combinations, not one each. The choice is about the
    /// *mechanism*, and a user who reaches for it is answering "my hotkey does
    /// nothing" — a question that has never been asked about one key and not
    /// the other, and two settings would only invite the half-configured state
    /// where the answer is yes for clips and no for screenshots.
    ///
    /// Deliberately absent from `REQUIRES_REARM`: the pump re-binds both keys
    /// the moment this changes, exactly as it does for a rebind of either
    /// combination.
    ///
    /// Defaulted to standard and left there. `"low_level"` is not a better
    /// mode, it is a different trade: it sees a combination another program
    /// has taken, and it does that by watching every keystroke on the machine,
    /// which is a shape some anti-cheat software distrusts. A bug we can fix
    /// in a patch; a ban we cannot undo. So nobody gets it without choosing
    /// it, and [`HotkeyMode::LowLevel`] carries the rest of the reasoning.
    #[serde(default = "default_hotkey_mode")]
    pub hotkey_mode: String,
}

/// Corner of the screen the on-screen banner shows in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BannerCorner {
    #[default]
    TopRight,
    TopLeft,
}

/// How many applications [`Config::audio_apps`] can name. Each one is an
/// audio stream the capture thread keeps open, so the list is bounded.
pub const MAX_AUDIO_APPS: usize = 8;

/// Cleans an application list into what the capture matches against: trimmed,
/// lower-cased, bare file names only, no duplicates, at most
/// [`MAX_AUDIO_APPS`].
///
/// The list is the only config value typed by hand into a file or sent by a
/// client, and the capture compares it to process names byte for byte, so
/// `"Spotify.exe "` or `"C:\\Apps\\Spotify.exe"` would match nothing and the
/// user would record silence. Paths are reduced to their file name rather than
/// rejected for the same reason: the meaning is clear and refusing it helps
/// nobody. Anything left empty is dropped.
pub fn normalize_audio_apps(apps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for app in apps {
        let name = app.rsplit(['\\', '/']).next().unwrap_or("").trim().to_lowercase();
        if name.is_empty() || out.contains(&name) {
            continue;
        }
        out.push(name);
        if out.len() == MAX_AUDIO_APPS {
            break;
        }
    }
    out
}

/// Lowest and highest supported capture/encode frame rate.
pub const MIN_FPS: u32 = 30;
pub const MAX_FPS: u32 = 120;
/// The widest/tallest side NVENC H.264 accepts.
const MAX_ENCODE_SIDE: u32 = 4096;

/// Output size for a monitor of `src_w` x `src_h` and a resolution preset.
///
/// `preset` is the length of the *short* side; the long side keeps the
/// monitor's aspect ratio, rounded to a multiple of 4 (NV12 needs even sizes
/// and decoders prefer a clean crop), and the result never exceeds
/// [`MAX_ENCODE_SIDE`]. `None` (native) returns the source untouched.
pub fn encode_size(src_w: u32, src_h: u32, preset: Option<u32>) -> (u32, u32) {
    let Some(preset) = preset else { return (src_w, src_h) };
    if src_w == 0 || src_h == 0 {
        return (src_w, src_h);
    }
    let portrait = src_w <= src_h;
    let (short, long) = if portrait { (src_w, src_h) } else { (src_h, src_w) };
    let round4 = |v: u64| (((v + 2) / 4) * 4).max(4) as u32;
    let mut short_out = preset;
    let mut long_out =
        round4((u64::from(long) * u64::from(preset) + u64::from(short) / 2) / u64::from(short));
    if long_out > MAX_ENCODE_SIDE {
        short_out = round4(u64::from(preset) * u64::from(MAX_ENCODE_SIDE) / u64::from(long_out));
        long_out = MAX_ENCODE_SIDE;
    }
    if portrait { (short_out, long_out) } else { (long_out, short_out) }
}

/// Bitrate (kbit/s) used when `bitrate_kbps = 0`. Anchored on the long-standing
/// 8 Mbit/s at 1920x1080 and 60 fps: it scales linearly with pixel count and
/// with the 0.8 power of the frame rate (consecutive frames at a high rate are
/// more alike, so they cost less than proportionally), within 2.5..50 Mbit/s.
pub fn auto_bitrate_kbps(width: u32, height: u32, fps: u32) -> u32 {
    const REFERENCE_KBPS: f64 = 8000.0;
    const REFERENCE_PIXELS: f64 = 1920.0 * 1080.0;
    const REFERENCE_FPS: f64 = 60.0;
    let pixels = f64::from(width) * f64::from(height);
    let kbps = REFERENCE_KBPS
        * (pixels / REFERENCE_PIXELS)
        * (f64::from(fps.max(1)) / REFERENCE_FPS).powf(0.8);
    (kbps.round() as u32).clamp(2_500, 50_000)
}

/// The quality levels of `video_quality`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoQuality {
    Low,
    Medium,
    High,
    VeryHigh,
    /// `bitrate_kbps` decides (0 = [`auto_bitrate_kbps`]).
    Custom,
}

impl VideoQuality {
    /// The level's bitrate at 1920x1080 and 60 fps, in kbit/s. The fixed
    /// tiers it replaces (5/8/14/20 Mbit/s whatever the fps) starved fast
    /// games at 120 fps; these sit near what NVIDIA's own presets spend.
    pub fn reference_kbps(self) -> Option<u32> {
        match self {
            Self::Low => Some(8_000),
            Self::Medium => Some(15_000),
            Self::High => Some(30_000),
            Self::VeryHigh => Some(50_000),
            Self::Custom => None,
        }
    }
}

/// Highest bitrate a quality level resolves to, in kbit/s. Keeps the 1.5x
/// VBR peak under the 200 Mbit/s bound `config.set` accepts.
pub const MAX_LEVEL_KBPS: u32 = 130_000;

/// A level's bitrate for an output of `width` x `height` at `fps`: the same
/// scaling as [`auto_bitrate_kbps`] (linear in pixels, 0.8 power of the frame
/// rate) from the level's own reference.
pub fn level_bitrate_kbps(level: VideoQuality, width: u32, height: u32, fps: u32) -> Option<u32> {
    let reference = f64::from(level.reference_kbps()?);
    let pixels = f64::from(width) * f64::from(height);
    let kbps =
        reference * (pixels / (1920.0 * 1080.0)) * (f64::from(fps.max(1)) / 60.0).powf(0.8);
    Some((kbps.round() as u32).clamp(2_500, MAX_LEVEL_KBPS))
}

/// One frame period in 100 ns units, rounded to nearest. Truncating (what the
/// engine did before) is up to 1 unit short, which for fps that do not divide
/// 10^7 (31, 33, 112...) biases the pacer slightly early.
pub fn frame_period_100ns(fps: u32) -> i64 {
    let fps = i64::from(fps.max(1));
    (10_000_000 + fps / 2) / fps
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 60,
            resolution: "native".into(),
            video_quality: "high".into(),
            bitrate_kbps: 0,
            max_bitrate_kbps: 0,
            rate_control: "vbr".into(),
            replay_seconds: 15,
            monitor_index: 0,
            clip_hotkey: "alt+f10".into(),
            gpu_priority: "low".into(),
            capture_method: "auto".into(),
            stats_seconds: 0,
            clip_dir: String::new(),
            max_library_gb: 0,
            system_volume: 100,
            mic_volume: 100,
            audio_apps: Vec::new(),
            capture_mode: "desktop".into(),
            game_apps: Vec::new(),
            group_by_game: true,
            session: SessionContext::default(),
            autostart: false,
            auto_arm: false,
            check_for_updates: false,
            auto_clip: false,
            auto_clip_min_kills: default_auto_clip_min_kills(),
            clip_sound: true,
            clip_sound_path: String::new(),
            discord_presence: true,
            screenshot_hotkey: default_screenshot_hotkey(),
            screenshot_sound: true,
            clip_banner: true,
            banner_position: default_banner_position(),
            notification_volume: default_notification_volume(),
            diagnostics: false,
            hotkey_mode: default_hotkey_mode(),
        }
    }
}

/// `serde(default)` for a bool yields `false`, which would silently turn the
/// update check off for every user upgrading from a config file written before
/// this key existed.
const fn default_auto_clip_min_kills() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_banner_position() -> String {
    "top_right".into()
}

const fn default_notification_volume() -> u32 {
    100
}

fn default_screenshot_hotkey() -> String {
    "alt+f8".into()
}

/// `serde(default)` for a `String` yields `""`, which would read as an unknown
/// value on every config file written before this key existed. That falls back
/// to standard anyway, but it would log a warning at every startup for a
/// choice the user never made.
fn default_hotkey_mode() -> String {
    "standard".into()
}

impl Config {
    /// Parsed rate-control mode (defaults to VBR on an unknown value).
    pub fn rate_control(&self) -> RateControl {
        match self.rate_control.trim().to_ascii_lowercase().as_str() {
            "cbr" => RateControl::Cbr,
            "vbr" => RateControl::PeakVbr,
            other => {
                tracing::warn!(rate_control = other, "unknown rate_control, using vbr");
                RateControl::PeakVbr
            }
        }
    }

    /// True unless the user explicitly opted into `gpu_priority = "normal"`.
    pub fn gpu_priority_low(&self) -> bool {
        match self.gpu_priority.trim().to_ascii_lowercase().as_str() {
            "normal" => false,
            "low" => true,
            other => {
                tracing::warn!(gpu_priority = other, "unknown gpu_priority, using low");
                true
            }
        }
    }

    /// What the replay records (defaults to `Desktop` on an unknown value, which
    /// is the way that cannot lose a clip someone expected).
    pub fn capture_mode(&self) -> CaptureMode {
        match self.capture_mode.trim().to_ascii_lowercase().as_str() {
            "desktop" => CaptureMode::Desktop,
            "game" => CaptureMode::Game,
            other => {
                tracing::warn!(capture_mode = other, "unknown capture_mode, using desktop");
                CaptureMode::Desktop
            }
        }
    }

    /// Which capture backend to use (defaults to `Auto` on an unknown value).
    pub fn capture_method(&self) -> CaptureMethod {
        match self.capture_method.trim().to_ascii_lowercase().as_str() {
            "auto" => CaptureMethod::Auto,
            "wgc" => CaptureMethod::Wgc,
            "dd" => CaptureMethod::Duplication,
            other => {
                tracing::warn!(capture_method = other, "unknown capture_method, using auto");
                CaptureMethod::Auto
            }
        }
    }

    /// How the hotkeys are listened for (defaults to `Standard` on an unknown
    /// value).
    ///
    /// The fallback direction is not arbitrary: a typo must never be what
    /// installs a keyboard hook on somebody's machine.
    pub fn hotkey_mode(&self) -> HotkeyMode {
        match self.hotkey_mode.trim().to_ascii_lowercase().as_str() {
            "standard" => HotkeyMode::Standard,
            "low_level" => HotkeyMode::LowLevel,
            other => {
                tracing::warn!(hotkey_mode = other, "unknown hotkey_mode, using standard");
                HotkeyMode::Standard
            }
        }
    }

    /// Preset height requested by `resolution`, `None` for native (or an
    /// unknown value, which falls back to native with a warning).
    pub fn resolution_preset(&self) -> Option<u32> {
        match self.resolution.trim().to_ascii_lowercase().as_str() {
            "native" | "" => None,
            "480p" => Some(480),
            "720p" => Some(720),
            "1080p" => Some(1080),
            "1440p" | "2k" => Some(1440),
            other => {
                tracing::warn!(resolution = other, "unknown resolution, using native");
                None
            }
        }
    }

    /// The audio tracks a clip carries, in order: the PC's sound (or just the
    /// named applications), then the microphone. Each is its own track so the
    /// editor can turn one down; a track whose level is 0 is simply absent.
    pub fn track_specs(&self) -> Vec<crate::capture::audio::TrackSpec> {
        use crate::capture::audio::{TrackKind, TrackSpec};
        if let Some(tracks) = &self.session.tracks {
            return tracks.clone();
        }
        let sound = if self.audio_apps.is_empty() {
            TrackSpec { name: "Son du PC".into(), kind: TrackKind::System }
        } else {
            TrackSpec {
                name: "Applications".into(),
                kind: TrackKind::Apps(self.audio_apps.clone()),
            }
        };
        vec![sound, TrackSpec { name: "Micro".into(), kind: TrackKind::Mic }]
    }

    /// The sub-folder this session's clips go in: the game's, `Bureau` on the
    /// desktop, or none at all (`""`) when grouping is switched off.
    pub fn clip_folder(&self) -> String {
        if !self.group_by_game {
            return String::new();
        }
        match &self.session.game {
            Some(game) if crate::library::is_valid_folder(&game.folder) => game.folder.clone(),
            _ => crate::library::DESKTOP_FOLDER.to_string(),
        }
    }

    /// The quality level. An empty value (a file from before the key) is
    /// read out of `bitrate_kbps`: the old fixed tiers map onto the level of
    /// the same name, the old auto (0) onto High, anything else is Custom so
    /// a typed bitrate is kept.
    pub fn video_quality(&self) -> VideoQuality {
        match self.video_quality.trim().to_ascii_lowercase().as_str() {
            "low" => VideoQuality::Low,
            "medium" => VideoQuality::Medium,
            "high" => VideoQuality::High,
            "very_high" => VideoQuality::VeryHigh,
            "custom" => VideoQuality::Custom,
            "" => match self.bitrate_kbps {
                0 | 14_000 => VideoQuality::High,
                5_000 => VideoQuality::Low,
                8_000 => VideoQuality::Medium,
                20_000 => VideoQuality::VeryHigh,
                _ => VideoQuality::Custom,
            },
            other => {
                tracing::warn!(video_quality = other, "unknown video_quality, using high");
                VideoQuality::High
            }
        }
    }

    /// Where the on-screen banner shows.
    pub fn banner_corner(&self) -> BannerCorner {
        match self.banner_position.trim().to_ascii_lowercase().as_str() {
            "top_left" => BannerCorner::TopLeft,
            _ => BannerCorner::TopRight,
        }
    }

    /// Target bitrate in bit/s for an output of `width` x `height`: the
    /// level's bitrate, or under Custom the explicit `bitrate_kbps` (the auto
    /// value when it is `0`).
    pub fn resolved_bitrate_bps(&self, width: u32, height: u32) -> u32 {
        if let Some(kbps) = level_bitrate_kbps(self.video_quality(), width, height, self.fps) {
            return kbps.saturating_mul(1000);
        }
        let kbps = if self.bitrate_kbps == 0 {
            auto_bitrate_kbps(width, height, self.fps)
        } else {
            self.bitrate_kbps
        };
        kbps.saturating_mul(1000)
    }

    /// VBR peak cap in bit/s for an output of `width` x `height`.
    /// `max_bitrate_kbps = 0` means auto (1.5x the resolved target); an
    /// explicit value is clamped to be at least the target.
    pub fn resolved_max_bitrate_bps(&self, width: u32, height: u32) -> u32 {
        let target = self.resolved_bitrate_bps(width, height);
        if self.max_bitrate_kbps == 0 {
            target.saturating_add(target / 2)
        } else {
            self.max_bitrate_kbps.saturating_mul(1000).max(target)
        }
    }

    pub fn path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("clippy").join("config.toml"))
    }

    /// Where the converted copy of the user's sound lives: beside the config
    /// file, so `%APPDATA%\clippy\config.toml` gives `%APPDATA%\clippy\clip-sound.wav`.
    ///
    /// Derived from the config path rather than from `%APPDATA%` directly, and
    /// that is the whole point: the daemon already carries a `config_path` that
    /// tests point at a scratch directory, so the cache follows it there
    /// without a second thing to remember to isolate.
    pub fn sound_cache_path(config_path: &Path) -> PathBuf {
        config_path.with_file_name("clip-sound.wav")
    }

    /// Where the cache's sidecar lives: which source file the cache at
    /// [`Self::sound_cache_path`] was actually built from.
    ///
    /// A cache file existing says nothing about *which* sound produced it --
    /// this is what lets `repair_sound_cache` tell those apart instead of
    /// trusting mere presence. Derived from `config_path` the same way the
    /// cache's own path is, and for the same reason: a scratch `config_path`
    /// in a test carries the sidecar with it too, with nothing extra to
    /// isolate.
    pub fn sound_src_path(config_path: &Path) -> PathBuf {
        config_path.with_file_name("clip-sound.src")
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => config,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "ignoring malformed config");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        };
        config.clamp_volumes();
        config.clamp_fps();
        config.audio_apps = normalize_audio_apps(&config.audio_apps);
        config.game_apps = normalize_audio_apps(&config.game_apps);
        config.settle_video_quality();
        config.notification_volume = config.notification_volume.min(100);
        config
    }

    /// Writes the level an old file implies into `video_quality`, so the
    /// settings page shows a real choice instead of an empty one, and an old
    /// fixed tier keeps meaning the level of the same name.
    pub fn settle_video_quality(&mut self) {
        if self.video_quality.trim().is_empty() {
            self.video_quality = match self.video_quality() {
                VideoQuality::Low => "low",
                VideoQuality::Medium => "medium",
                VideoQuality::High => "high",
                VideoQuality::VeryHigh => "very_high",
                VideoQuality::Custom => "custom",
            }
            .into();
        }
    }

    /// Clamps the two capture levels to their documented 0..=200 range.
    ///
    /// `config.set` enforces this range itself (`state.rs`'s bounds table), but
    /// a hand-edited `config.toml` bypasses that check entirely — `toml`
    /// deserializes `mic_volume = 500` into a `u32` without complaint, since
    /// nothing at the type level says otherwise. Left unclamped, `config.get`
    /// would report 500 and the Settings page would render "500%" next to a
    /// slider pinned at its maximum, while `AudioGains::new`'s own clamp
    /// quietly capped what capture actually applied — display and reality would
    /// disagree. This is the one place that has to catch a value nothing else
    /// validates.
    ///
    /// The ceiling is [`MAX_VOLUME_PERCENT`], imported rather than written out,
    /// so this and the gain curve can never be raised independently of each
    /// other.
    fn clamp_volumes(&mut self) {
        self.system_volume = self.system_volume.min(MAX_VOLUME_PERCENT);
        self.mic_volume = self.mic_volume.min(MAX_VOLUME_PERCENT);
    }

    /// Same hand-edited-file guard for `fps`: `MIN_FPS..=MAX_FPS`.
    fn clamp_fps(&mut self) {
        let clamped = self.fps.clamp(MIN_FPS, MAX_FPS);
        if clamped != self.fps {
            tracing::warn!(fps = self.fps, clamped, "fps outside the supported range, clamped");
            self.fps = clamped;
        }
    }

    /// Resolved clip directory (spec §5.1). A flat, timestamped folder —
    /// manual arming means there is no game name to fold on, and a flat
    /// directory sorts correctly in Explorer for people who never open the UI.
    pub fn clip_dir_path(&self) -> PathBuf {
        let configured = self.clip_dir.trim();
        if !configured.is_empty() {
            return PathBuf::from(configured);
        }
        std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Videos").join("Clippy"))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Writes the config to `path`, creating the directory. Used by
    /// `config.set`.
    ///
    /// Takes the path rather than deriving `%APPDATA%\clippy\config.toml`
    /// itself, and there used to be a `save()` above that did the deriving —
    /// it is gone because nothing called it. The daemon holds its own
    /// `config_path` (`state::Daemon`, resolved once from [`Self::path`] at
    /// construction) precisely so a test can persist somewhere other than the
    /// developer's real settings file, the same seam `pipe::serve_at` is to
    /// `pipe::serve`. With every caller already holding a path, a second
    /// entry point that quietly picked the real one was a trap rather than a
    /// convenience.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("sérialisation de la configuration")?;
        std::fs::write(path, text).with_context(|| format!("écriture de {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_size_keeps_the_monitor_aspect_ratio() {
        assert_eq!(encode_size(1920, 1080, None), (1920, 1080));
        assert_eq!(encode_size(1920, 1080, Some(480)), (852, 480));
        assert_eq!(encode_size(1920, 1080, Some(720)), (1280, 720));
        assert_eq!(encode_size(1920, 1080, Some(1080)), (1920, 1080));
        assert_eq!(encode_size(1920, 1080, Some(1440)), (2560, 1440));
        assert_eq!(encode_size(1920, 1200, Some(720)), (1152, 720), "16:10");
        assert_eq!(encode_size(3440, 1440, Some(1080)), (2580, 1080), "21:9");
        assert_eq!(encode_size(1080, 1920, Some(720)), (720, 1280), "portrait");
    }

    #[test]
    fn encode_size_is_always_even_capped_and_non_empty() {
        for (w, h) in [(1920, 1080), (2560, 1440), (3840, 2160), (5120, 1440), (1366, 768)] {
            for preset in [480, 720, 1080, 1440] {
                let (ow, oh) = encode_size(w, h, Some(preset));
                assert!(ow >= 4 && oh >= 4, "{w}x{h}@{preset} gave {ow}x{oh}");
                assert!(ow % 2 == 0 && oh % 2 == 0, "{w}x{h}@{preset} gave {ow}x{oh}");
                assert!(ow <= MAX_ENCODE_SIDE && oh <= MAX_ENCODE_SIDE, "{ow}x{oh}");
            }
        }
        assert_eq!(encode_size(0, 0, Some(720)), (0, 0));
    }

    #[test]
    fn resolution_presets_parse_and_unknown_means_native() {
        let mut config = Config::default();
        assert_eq!(config.resolution_preset(), None);
        for (text, preset) in
            [("480p", 480), ("720P", 720), ("1080p", 1080), ("1440p", 1440), ("2k", 1440)]
        {
            config.resolution = text.into();
            assert_eq!(config.resolution_preset(), Some(preset), "{text}");
        }
        config.resolution = "4k".into();
        assert_eq!(config.resolution_preset(), None);
    }

    #[test]
    fn auto_bitrate_matches_the_reference_and_scales_sensibly() {
        assert_eq!(auto_bitrate_kbps(1920, 1080, 60), 8000);
        assert_eq!(auto_bitrate_kbps(852, 480, 30), 2500, "floor");
        let high = auto_bitrate_kbps(2560, 1440, 120);
        assert!((20_000..=30_000).contains(&high), "1440p120 got {high}");
        assert!(auto_bitrate_kbps(1920, 1080, 120) > auto_bitrate_kbps(1920, 1080, 60));
        assert!(auto_bitrate_kbps(7680, 4320, 120) <= 50_000, "cap");
    }

    #[test]
    fn quality_levels_follow_resolution_and_fps() {
        let mut config = Config { video_quality: "high".into(), ..Config::default() };
        config.fps = 60;
        assert_eq!(config.resolved_bitrate_bps(1920, 1080), 30_000_000);
        config.fps = 120;
        let at_120 = config.resolved_bitrate_bps(1920, 1080);
        assert!((50_000_000..=55_000_000).contains(&at_120), "1080p120 high got {at_120}");
        config.video_quality = "very_high".into();
        assert!(config.resolved_bitrate_bps(3840, 2160) <= MAX_LEVEL_KBPS * 1000, "cap");
        assert!(config.resolved_max_bitrate_bps(3840, 2160) <= 200_000_000);
        config.video_quality = "low".into();
        config.fps = 30;
        assert_eq!(config.resolved_bitrate_bps(640, 360), 2_500_000, "floor");
    }

    #[test]
    fn a_file_without_video_quality_keeps_its_choice() {
        let level = |kbps: u32| {
            Config { video_quality: String::new(), bitrate_kbps: kbps, ..Config::default() }
                .video_quality()
        };
        assert_eq!(level(0), VideoQuality::High);
        assert_eq!(level(5_000), VideoQuality::Low);
        assert_eq!(level(8_000), VideoQuality::Medium);
        assert_eq!(level(14_000), VideoQuality::High);
        assert_eq!(level(20_000), VideoQuality::VeryHigh);
        assert_eq!(level(12_345), VideoQuality::Custom);
        let mut old: Config = toml::from_str("bitrate_kbps = 14000\n").unwrap();
        assert_eq!(old.video_quality(), VideoQuality::High);
        old.settle_video_quality();
        assert_eq!(old.video_quality, "high");
        let mut typed: Config = toml::from_str("bitrate_kbps = 12345\n").unwrap();
        typed.settle_video_quality();
        assert_eq!(typed.video_quality, "custom");
        assert_eq!(old.notification_volume, 100);
        assert_eq!(old.banner_corner(), BannerCorner::TopRight);
        let custom =
            Config { video_quality: "custom".into(), bitrate_kbps: 12_000, ..Config::default() };
        assert_eq!(custom.resolved_bitrate_bps(1920, 1080), 12_000_000);
    }

    #[test]
    fn explicit_bitrate_wins_over_auto() {
        let mut config = Config { video_quality: "custom".into(), ..Config::default() };
        assert_eq!(config.resolved_bitrate_bps(1920, 1080), 8_000_000);
        assert_eq!(config.resolved_max_bitrate_bps(1920, 1080), 12_000_000);
        config.bitrate_kbps = 5000;
        assert_eq!(config.resolved_bitrate_bps(2560, 1440), 5_000_000);
        assert_eq!(config.resolved_max_bitrate_bps(2560, 1440), 7_500_000);
    }

    #[test]
    fn frame_period_rounds_to_nearest() {
        assert_eq!(frame_period_100ns(60), 166_667);
        assert_eq!(frame_period_100ns(120), 83_333);
        assert_eq!(frame_period_100ns(31), 322_581);
        assert_eq!(frame_period_100ns(33), 303_030);
        assert_eq!(frame_period_100ns(112), 89_286);
        assert_eq!(frame_period_100ns(0), 10_000_000, "never divides by zero");
    }

    #[test]
    fn fps_is_clamped_to_the_supported_range() {
        let mut config = Config { fps: 500, ..Config::default() };
        config.clamp_fps();
        assert_eq!(config.fps, MAX_FPS);
        config.fps = 5;
        config.clamp_fps();
        assert_eq!(config.fps, MIN_FPS);
        config.fps = 97;
        config.clamp_fps();
        assert_eq!(config.fps, 97, "in-range values are untouched");
    }

    /// The default must be `Auto`, and `Auto` must be WGC. Desktop Duplication
    /// has no mouse cursor, so a release that made it anybody's default without
    /// them asking would silently take the pointer out of their clips.
    #[test]
    fn capture_method_defaults_to_auto() {
        assert_eq!(Config::default().capture_method(), CaptureMethod::Auto);
    }

    /// `config.toml` is hand-editable, so every value here is something a user
    /// can actually type. An unrecognised one must start on the shipped
    /// backend rather than refuse to arm -- the same contract `rate_control`
    /// and `gpu_priority` already keep.
    #[test]
    fn an_unreadable_capture_method_falls_back_to_auto_rather_than_failing() {
        let method =
            |raw: &str| Config { capture_method: raw.into(), ..Config::default() }.capture_method();
        assert_eq!(method("wgc"), CaptureMethod::Wgc);
        assert_eq!(method("dd"), CaptureMethod::Duplication);
        assert_eq!(method("  DD  "), CaptureMethod::Duplication, "trimmed and case-folded");
        assert_eq!(
            method("duplication"),
            CaptureMethod::Auto,
            "a plausible guess is still unknown"
        );
        assert_eq!(method(""), CaptureMethod::Auto);
        assert_eq!(method("dxgi"), CaptureMethod::Auto);
    }

    /// The one fallback in this file where the direction is a safety property
    /// rather than a convenience: standard mode reserves a combination,
    /// low-level mode watches the whole keyboard, and a value nobody
    /// recognises must never be what turns the second one on.
    #[test]
    fn an_unreadable_hotkey_mode_falls_back_to_standard() {
        let mode =
            |raw: &str| Config { hotkey_mode: raw.into(), ..Config::default() }.hotkey_mode();
        assert_eq!(Config::default().hotkey_mode(), HotkeyMode::Standard);
        assert_eq!(mode("low_level"), HotkeyMode::LowLevel);
        assert_eq!(mode("  LOW_LEVEL  "), HotkeyMode::LowLevel, "trimmed and case-folded");
        assert_eq!(mode("low-level"), HotkeyMode::Standard, "a plausible guess is still unknown");
        assert_eq!(mode("hook"), HotkeyMode::Standard);
        assert_eq!(mode(""), HotkeyMode::Standard);
    }

    #[test]
    fn clip_dir_defaults_under_the_user_profile() {
        let config = Config::default();
        let path = config.clip_dir_path();
        assert!(path.ends_with(r"Videos\Clippy"), "unexpected default: {}", path.display());
    }

    #[test]
    fn an_explicit_clip_dir_wins() {
        let config = Config { clip_dir: r"D:\Clips".into(), ..Config::default() };
        assert_eq!(config.clip_dir_path(), PathBuf::from(r"D:\Clips"));
    }

    /// `deny_unknown_fields` means a config written by a newer build must not
    /// be silently ignored — and a config from before `clip_dir` existed must
    /// still load.
    #[test]
    fn a_config_without_clip_dir_still_loads() {
        let config: Config = toml::from_str("fps = 30\nreplay_seconds = 20\n").unwrap();
        assert_eq!(config.fps, 30);
        assert_eq!(config.replay_seconds, 20);
        assert_eq!(config.clip_dir, "");
    }

    /// Stage 3's two new keys (spec §5.4, §7.3). `deny_unknown_fields` is set,
    /// so the upgrade path matters: a `config.toml` written before these
    /// existed must still load rather than being reported malformed and
    /// silently replaced by defaults — which would discard the user's whole
    /// configuration on first run of a new build.
    #[test]
    fn the_stage_three_keys_default_and_an_older_config_still_loads() {
        let config = Config::default();
        assert_eq!(config.max_library_gb, 0, "spec §5.4: the ceiling is opt-in");
        assert!(!config.autostart, "spec §7.3: opt-in, off by default");

        let older: Config = toml::from_str("fps = 30\nreplay_seconds = 20\n").unwrap();
        assert_eq!(older.fps, 30, "the user's real settings must survive");
        assert_eq!(older.max_library_gb, 0);
        assert!(!older.autostart);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let original = Config { clip_dir: r"D:\Clips".into(), fps: 30, ..Config::default() };
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.fps, 30);
        assert_eq!(parsed.clip_dir, r"D:\Clips");
    }

    #[test]
    fn both_levels_default_to_full() {
        let config = Config::default();
        assert_eq!(config.system_volume, 100);
        assert_eq!(config.mic_volume, 100);
    }

    #[test]
    fn a_config_file_without_the_levels_still_loads_at_full() {
        // Every config.toml written before this feature existed lacks both
        // keys. Serde's `default` has to cover them or upgrading silently
        // mutes everyone.
        let config: Config = toml::from_str("fps = 60\n").expect("a partial config still parses");
        assert_eq!(config.system_volume, 100);
        assert_eq!(config.mic_volume, 100);
    }

    #[test]
    fn an_explicit_zero_survives_the_defaulting() {
        // The one value `#[serde(default)]` could plausibly eat. 0 is not
        // "unset" here -- it is how the user turns a source off, and for the
        // microphone it is the *only* way, since there is no separate toggle.
        // Silently restoring 100 would reopen the stream and put the Windows
        // microphone indicator back in the taskbar of someone who switched it
        // off on purpose.
        let config: Config = toml::from_str("system_volume = 0\nmic_volume = 0\n")
            .expect("an explicit zero still parses");
        assert_eq!(config.system_volume, 0);
        assert_eq!(config.mic_volume, 0);
    }

    /// `config.set` refuses an out-of-range level by name (`state.rs`'s bounds
    /// table), but that check has no say over a `config.toml` edited by hand.
    /// `toml::from_str` happily parses `mic_volume = 500` into a `u32` --
    /// `clamp_volumes` (called from `Config::load`, since `load` resolves its
    /// path from `%APPDATA%` and cannot be pointed at a fixture here) is the
    /// only place left to catch it, and it has to, or the Settings page
    /// renders "500%" next to a slider capture never actually reaches
    /// (`AudioGains::new` clamps regardless).
    #[test]
    fn a_hand_edited_out_of_range_level_is_clamped() {
        let mut config: Config = toml::from_str("system_volume = 500\nmic_volume = 9001\n")
            .expect("an out-of-range level still parses");
        assert_eq!(config.system_volume, 500, "unclamped straight out of toml::from_str");
        config.clamp_volumes();
        assert_eq!(
            config.system_volume, MAX_VOLUME_PERCENT,
            "500% must clamp to the documented maximum"
        );
        assert_eq!(
            config.mic_volume, MAX_VOLUME_PERCENT,
            "9001% must clamp to the documented maximum"
        );
    }

    /// A level inside the new boost range survives the clamp untouched. The
    /// test above only proves the ceiling holds; this one proves the ceiling
    /// moved, which is the change nobody would notice breaking.
    #[test]
    fn a_boosted_level_is_left_alone() {
        let mut config: Config = toml::from_str("system_volume = 200\nmic_volume = 140\n")
            .expect("a boosted level parses");
        config.clamp_volumes();
        assert_eq!(config.system_volume, 200);
        assert_eq!(config.mic_volume, 140);
    }

    /// `serde(default)` on a bool is `false`, so a config file written before
    /// these keys existed would silently turn the sound off for every upgrading
    /// user. `check_for_updates` learned this the same way.
    #[test]
    fn clip_sound_defaults_on_for_a_config_written_before_it_existed() {
        let old = "fps = 60\nbitrate_kbps = 8000\n";
        let config: Config = toml::from_str(old).expect("an old config must still parse");
        assert!(config.clip_sound, "a missing clip_sound must read as on");
        assert_eq!(config.clip_sound_path, "", "a missing path must read as the built-in");
    }

    #[test]
    fn the_sound_keys_round_trip_through_toml() {
        let config = Config {
            clip_sound: false,
            clip_sound_path: r"C:\Users\someone\airhorn.mp3".into(),
            ..Config::default()
        };
        let round: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert!(!round.clip_sound);
        assert_eq!(round.clip_sound_path, r"C:\Users\someone\airhorn.mp3");
    }

    /// The cache is a sibling of the config file, which is what makes it scratch
    /// in tests for free: a `Daemon` built with a scratch `config_path` gets a
    /// scratch cache without a second seam to remember.
    #[test]
    fn the_sound_cache_sits_beside_the_config_file() {
        let cache = Config::sound_cache_path(std::path::Path::new(r"C:\x\clippy\config.toml"));
        assert_eq!(cache, std::path::PathBuf::from(r"C:\x\clippy\clip-sound.wav"));
    }

    /// Same derivation as the cache itself, so a scratch `config_path` in a
    /// test isolates both together.
    #[test]
    fn the_sound_sidecar_sits_beside_the_config_file() {
        let sidecar = Config::sound_src_path(std::path::Path::new(r"C:\x\clippy\config.toml"));
        assert_eq!(sidecar, std::path::PathBuf::from(r"C:\x\clippy\clip-sound.src"));
    }

    #[test]
    fn audio_apps_are_cleaned_into_bare_lowercase_names() {
        let raw = [
            " Spotify.EXE ".to_string(),
            r"C:\Apps\Spotify.exe".to_string(),
            "".to_string(),
            "   ".to_string(),
            "C:/Program Files/Brave/brave.exe".to_string(),
        ];
        assert_eq!(normalize_audio_apps(&raw), vec!["spotify.exe", "brave.exe"]);
    }

    #[test]
    fn the_capture_mode_defaults_to_the_desktop_and_falls_back_to_it() {
        assert_eq!(Config::default().capture_mode(), CaptureMode::Desktop);
        let game = Config { capture_mode: " Game ".into(), ..Config::default() };
        assert_eq!(game.capture_mode(), CaptureMode::Game);
        let typo = Config { capture_mode: "gmae".into(), ..Config::default() };
        assert_eq!(typo.capture_mode(), CaptureMode::Desktop);
    }

    #[test]
    fn audio_apps_stop_at_the_documented_ceiling() {
        let raw: Vec<String> = (0..MAX_AUDIO_APPS + 5).map(|i| format!("app{i}.exe")).collect();
        assert_eq!(normalize_audio_apps(&raw).len(), MAX_AUDIO_APPS);
    }

    #[test]
    fn the_default_records_every_application() {
        assert!(Config::default().audio_apps.is_empty());
        let parsed: Config = toml::from_str("audio_apps = [\"spotify.exe\"]\n").expect("parses");
        assert_eq!(parsed.audio_apps, vec!["spotify.exe"]);
    }
}
