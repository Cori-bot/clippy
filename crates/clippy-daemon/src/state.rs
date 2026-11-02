//! Everything the daemon owns, and the rules for changing it.
//!
//! Mostly transport-free: nothing here knows about pipes or requests, and for
//! most methods [`crate::dispatch`] turns the return value into a response and
//! decides what to broadcast. `record_saved_clip`, `record_saved_shot`, `arm`
//! and `disarm` are the exceptions, and deliberately so: each of them is
//! reached by a path that never passes through `dispatch` at all — a hotkey
//! clip or screenshot calls straight into `Daemon::clip` / `Daemon::screenshot`,
//! and the tray menu calls straight into `Daemon::arm` / `Daemon::disarm` — so
//! `clip_saved`, `shot_saved`, `armed` and `disarmed` have to be broadcast from
//! the one function every path actually goes through. See `record_saved_clip`'s
//! comment for the bug that made that structural rather than a convention to
//! remember, and `arm`'s for the second time the same bug arrived. All four
//! broadcasts still happen after the `armed` lock is released, for the same
//! reason every other event does, which is what keeps the lock ordering below
//! trivially true regardless of which module is doing the broadcasting.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use clippy_core::{
    capture::audio::{AudioGains, MAX_VOLUME_PERCENT, TrackKind, TrackSpec},
    config::{CaptureMode, Config, GameContext, MAX_FPS, MIN_FPS, SessionContext},
    control,
    control::SingleInstance,
    engine::EngineHandle,
    engine::EngineStatus,
    export, library, shot, stats,
    store::Store,
};
use clippy_proto::{ClipMeta, Event, ShotMeta};
use serde_json::{Map, Value};

use crate::clients::Clients;
use crate::game::{Lists, Sighting};
use crate::games::{Edit, GameEntry, GameProfile, Registry};

/// The engine plus the single-instance slot it holds. Armed as a unit,
/// disarmed as a unit — dropping this releases the encoder *and* the slot, so
/// `clippy replay` from a terminal works again the moment the daemon disarms.
struct Armed {
    engine: EngineHandle,
    _slot: SingleInstance,
}

/// How long a game may be out of sight before it counts as gone.
const GAME_GRACE: Duration = Duration::from_secs(5);

/// Spacing between attempts to start the replay for a game when starting fails,
/// so a broken encoder is retried calmly and not every second.
const GAME_RETRY: Duration = Duration::from_secs(15);

/// What the `game` capture mode knows. A leaf lock: nothing else is taken while
/// it is held, except `armed` for a read in `game_tick`, which is the order
/// everything else uses (`armed` may be held while this is taken, and the
/// reverse is only ever the single read noted there).
#[derive(Default)]
struct GameWatch {
    /// The replay was asked for. It runs when a game is in front, or at once in
    /// `desktop` mode; cleared by `disarm`.
    wanted: bool,
    /// The engine is running because a game is in front, and stops with it.
    started_by_game: bool,
    /// The game in front or still running, or `None` after [`GAME_GRACE`].
    sighting: Option<Sighting>,
    last_seen: Option<Instant>,
    last_attempt: Option<Instant>,
    /// When the registry was last told the game in front is running, so
    /// `last_played` is refreshed about once a minute and not every second.
    last_noted: Option<(String, Instant)>,
}

/// How often a running game's `last_played` is rewritten.
const GAME_NOTE_EVERY: Duration = Duration::from_secs(60);

/// What `status` answers with, and the payload of the `armed` event.
///
/// A struct rather than a bare `serde_json::Value` so this module's tests can
/// assert on fields, but [`Self::to_json`] is the only thing that reaches the
/// wire and its key set is frozen by spec §4.2: `armed`, `encoder`,
/// `monitor_index`, `ring_seconds_used`, `ring_seconds_total`, `version`,
/// `clip_dir`. A UI binds to those names, so they do not get to drift — see
/// `the_status_wire_shape_is_the_published_one` below.
pub struct DaemonStatus {
    pub armed: bool,
    /// `None` when disarmed — and also, for up to one 250 ms engine tick right
    /// after `arm` returns, when armed. [`EngineHandle::spawn`] returns as soon
    /// as the first capture session is live, but `EngineStatus.encoder` is only
    /// published on the engine's control tick, so a `status` (or an `armed`
    /// event) issued immediately after arming can legitimately carry `null`
    /// here. The next `status` has the real name. Closing that window wants a
    /// liveness field on the engine itself, which is Task 7's job — guessing a
    /// name here would be worse than reporting "not known yet".
    pub encoder: Option<String>,
    pub monitor_index: u32,
    pub ring_seconds_used: f64,
    pub ring_seconds_total: u32,
    pub clip_dir: String,
    /// The library cache: `thumbs`, `tracks` and `shots` under it hold the
    /// thumbnails and editor audio kept out of the clips folder.
    pub cache_dir: String,
    /// Whether the clip combination is actually listening — `None` until the
    /// pump has tried, which is a real answer and not a defaulted one.
    ///
    /// Added after the fact, and the key set above is described as frozen, so
    /// to be exact about what that means: *renaming or dropping* a published
    /// key breaks a client, adding one cannot. A client that has never heard
    /// of this field ignores it and behaves as it always did.
    ///
    /// It is here because the alternative was nothing. A hotkey Windows
    /// refuses — another program owning Alt+F10, which NVIDIA's overlay does
    /// on a great many machines — produced one `tracing::warn!` into a log
    /// with no file behind it, and the user's whole experience was a key that
    /// did nothing, forever, with no way to find out why.
    pub clip_hotkey_bound: Option<bool>,
    /// The same answer for the screenshot combination. Separate rather than
    /// one "hotkeys are fine" flag: they are bound independently and either
    /// can lose to a different program, so a single flag could only ever say
    /// which is *worse*, and the settings page has to mark one row.
    pub screenshot_hotkey_bound: Option<bool>,
    /// The replay is on but idle, waiting for a game (`game` capture mode).
    /// Distinct from `armed: false`, which means the user has not turned it on:
    /// a UI shows "waiting for a game" for one and "off" for the other.
    pub waiting_for_game: bool,
    /// The program of the game in front or still running, when there is one.
    pub game: Option<String>,
    /// The name to show for that game.
    pub game_name: Option<String>,
}

impl DaemonStatus {
    /// The wire object, built with explicit `Value::from` conversions rather
    /// than `serde_json::json!` or a `Serialize` derive: every arm of that
    /// macro other than its `true`/`false`/`null`/array/object literals expands
    /// to `serde_json::to_value(&x).unwrap()`, and this runs on a path a socket
    /// message reaches directly in a `panic = "abort"` build. The `From` impls
    /// used here (integers, `f64`, `&str`) cannot fail, so there is nothing to
    /// unwrap.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Value};
        let mut fields = Map::new();
        fields.insert("armed".to_string(), Value::Bool(self.armed));
        fields.insert(
            "encoder".to_string(),
            match &self.encoder {
                Some(name) => Value::from(name.as_str()),
                None => Value::Null,
            },
        );
        fields.insert("monitor_index".to_string(), Value::from(self.monitor_index));
        fields.insert("ring_seconds_used".to_string(), Value::from(self.ring_seconds_used));
        fields.insert("ring_seconds_total".to_string(), Value::from(self.ring_seconds_total));
        fields.insert("version".to_string(), Value::from(env!("CARGO_PKG_VERSION")));
        fields.insert("clip_dir".to_string(), Value::from(self.clip_dir.as_str()));
        fields.insert("cache_dir".to_string(), Value::from(self.cache_dir.as_str()));
        // `null` for "not tried yet", the same three-way shape `encoder` above
        // already uses, and for the same reason: a client must be able to tell
        // "no" from "not known", because only one of them is worth telling the
        // user about.
        for (key, bound) in [
            ("clip_hotkey_bound", self.clip_hotkey_bound),
            ("screenshot_hotkey_bound", self.screenshot_hotkey_bound),
        ] {
            fields.insert(
                key.to_string(),
                match bound {
                    Some(bound) => Value::Bool(bound),
                    None => Value::Null,
                },
            );
        }
        fields.insert("waiting_for_game".to_string(), Value::Bool(self.waiting_for_game));
        for (key, value) in [("game", &self.game), ("game_name", &self.game_name)] {
            fields.insert(
                key.to_string(),
                match value {
                    Some(name) => Value::from(name.as_str()),
                    None => Value::Null,
                },
            );
        }
        Value::Object(fields)
    }
}

/// One page of screenshots, newest first, plus what a UI needs to render "3 of
/// 47".
///
/// Shares its shape with [`LibraryPage`] rather than its doc comment: read
/// that struct's comment for why `total` and `offset` are carried at all.
pub struct ShotPage {
    pub shots: Vec<ShotMeta>,
    pub total: usize,
    pub offset: usize,
}

/// One page of the clip library, plus what a UI needs to render "3 of 47".
///
/// `total` is the *unpaged* count, not `clips.len()`: without it a client that
/// asked for 50 and got 50 cannot tell a full library from a full page, and
/// cannot size a scrollbar or a page count. `offset` is echoed back because
/// responses are matched by request id, not by argument — a client that has
/// several pages in flight would otherwise have to remember which id asked for
/// which page.
pub struct LibraryPage {
    pub clips: Vec<ClipMeta>,
    pub total: usize,
    pub offset: usize,
    pub total_bytes: u64,
    /// Opaque identity of the cached metadata, shared by all pages of one scan.
    pub snapshot: String,
}

/// A sealed home for [`ClipPaths`], so the guarantee below is enforced by the
/// compiler against *this whole file*, not just against callers outside it.
///
/// A plain `struct ClipPaths { .. }` sitting directly in `state.rs` would not
/// be enough: private fields are visible to every function in the module that
/// declares them, so a fifth `impl Daemon` method added later — in this same
/// file — could still write a `ClipPaths { mp4: .., .. }` struct literal by
/// hand and skip validation entirely, and the compiler would not object. Only
/// a *child* module's private items are hidden from its parent, so nesting
/// the struct one level down and keeping its fields private to that nested
/// module is what actually closes the loophole: `state.rs` can call
/// [`ClipPaths::for_id`] and read the accessors, but it cannot see the fields
/// to construct one itself.
mod clip_paths {
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result, bail};
    use clippy_core::{library, store::Store};

    /// Already-built, already-validated paths for one clip id.
    ///
    /// The only way to get one is [`ClipPaths::for_id`], which checks the id
    /// before building a single path and finds the clip's file in the library
    /// index: the file is named freely ("VALORANT 2026-11-02 14-27-22.mp4"),
    /// so its path comes from the record, never from the id. The thumbnail and
    /// the editor's audio live in the library cache.
    pub struct ClipPaths {
        id: String,
        mp4: PathBuf,
        store: Store,
    }

    impl ClipPaths {
        pub(super) fn for_id(store: &Store, id: &str) -> Result<Self> {
            if !library::is_valid_id(id) {
                bail!("{id:?} n’est pas un identifiant de clip valide");
            }
            let entry = store.entry(id).with_context(|| format!("clip {id} introuvable"))?;
            Ok(Self { id: id.to_string(), mp4: entry.path, store: store.clone() })
        }

        /// The folder the clip's file is in.
        pub(super) fn dir(&self) -> &Path {
            self.mp4.parent().unwrap_or(Path::new(""))
        }

        pub(super) fn mp4(&self) -> &Path {
            &self.mp4
        }

        pub(super) fn thumb(&self) -> PathBuf {
            self.store.thumb(&self.id)
        }

        pub(super) fn track_cache(&self) -> PathBuf {
            self.store.tracks_dir()
        }

        pub(super) fn track(&self, ordinal: usize) -> PathBuf {
            self.store.track(&self.id, ordinal)
        }

        pub(super) fn track_partial(&self, ordinal: usize) -> PathBuf {
            self.store.track_partial(&self.id, ordinal)
        }
    }
}
use clip_paths::ClipPaths;

/// What [`Daemon::arm`] did.
///
/// No `newly_armed` flag: it existed so the dispatcher could broadcast `armed`
/// only on a real transition, and now that `arm` broadcasts for itself, the
/// early return for an already-armed daemon *is* that decision — it leaves
/// before reaching the broadcast. A UI that reconnects and re-arms to sync its
/// own toggle still makes no other client redraw.
pub struct ArmOutcome {
    pub status: DaemonStatus,
}

/// The config keys that only take effect at the next `arm`.
///
/// Every one is baked into `RecorderSettings` or the capture session when
/// `EngineHandle::spawn` builds it, so setting one while armed changes the file
/// and nothing else. `config.set` *reports* them rather than acting on them:
/// silently re-arming would tear the live replay ring down — and with it the
/// last fifteen seconds the user may be about to clip — because someone nudged
/// a bitrate slider. The UI decides whether to prompt.
///
/// The config keys deliberately absent take effect immediately: `clip_dir` is
/// read per clip, `stats_seconds` per stats tick, and `clip_hotkey` and
/// `screenshot_hotkey` are re-registered live by `window::rebind_hotkey`
/// rather than requiring a re-arm. `screenshot_sound` is read fresh by
/// `record_saved_shot` on every screenshot, the same way `clip_sound` already
/// is for clips.
pub(crate) const REQUIRES_REARM: [&str; 12] = [
    "fps",
    // The encoded size is fixed when the session opens.
    "resolution",
    "video_quality",
    "bitrate_kbps",
    "max_bitrate_kbps",
    "rate_control",
    "replay_seconds",
    "monitor_index",
    "gpu_priority",
    // How frames come off the screen is decided when the session opens: the
    // sink's converter and encoder are built on the backend's own device.
    "capture_method",
    // Which applications are captured is decided when the audio streams open.
    "audio_apps",
    // The folder a session files its clips in is fixed when it starts.
    "group_by_game",
];

/// The two capture levels, which follow a different re-arm rule from every
/// other key.
const VOLUME_KEYS: [&str; 2] = ["system_volume", "mic_volume"];

/// True when a level moved onto or off zero.
///
/// Levels apply live: the capture session reads them from a shared cell every
/// pump, so `40 → 70` is audible in the next clip with no re-arm. The
/// exception is zero, which is not a quiet level but a closed capture stream —
/// and streams only open at arm time. Reporting those two transitions, and
/// only those, is what lets the settings page prompt exactly when a prompt
/// changes the outcome.
///
/// A non-numeric value on either side is not a crossing: it is a type error,
/// and `set_config`'s per-key retry produces a far better message for it.
fn crosses_zero(key: &str, before: Option<&Value>, after: Option<&Value>) -> bool {
    if !VOLUME_KEYS.contains(&key) {
        return false;
    }
    let (Some(before), Some(after)) =
        (before.and_then(Value::as_u64), after.and_then(Value::as_u64))
    else {
        return false;
    };
    (before == 0) != (after == 0)
}

/// The accepted range of every numeric config key, as `(key, min, max)`.
///
/// Sanity bounds, not a product policy. Their job is to reject the absurd —
/// the values that turn one socket line into a machine-wide failure — and to
/// leave every value a person might plausibly want alone. Serde's
/// `from_value` is airtight about *type* and says nothing about *magnitude*,
/// and `replay_seconds` proves why that gap matters: `{"replay_seconds":
/// 4294967295}` deserializes cleanly, persists, and at the next `arm` becomes a
/// 136-year retention window, so `replay.rs`'s eviction never evicts and the
/// ring grows until the process is OOM-killed. `bitrate_kbps` blows the same
/// RAM bound from the other side, since the ring's size is seconds × bitrate.
/// The precedent is `MAX_LIST_LIMIT` in `clippy-proto`, which clamps
/// `library.list` for the same reason: a number off the wire that nothing else
/// bounds.
///
/// Why each number:
/// - `fps` 30..=120 — the range the engine is tuned and tested for (any
///   integer in it, independent of the resolution).
/// - `bitrate_kbps` 0..=200_000 — 200 Mbit/s is far above any real capture;
///   0 is the documented "auto" (derived from resolution and fps).
/// - `max_bitrate_kbps` 0..=200_000 — 0 stays legal because it is the
///   documented "auto" (1.5× target), and is the default.
/// - `replay_seconds` 1..=600 — the ring is RAM, so ten minutes is the outer
///   bound; 0 would be a ring with nothing in it to clip.
/// - `monitor_index` 0..=63 — more attached displays than any consumer GPU
///   drives, and an out-of-range index only fails at `arm` anyway.
/// - `stats_seconds` 0..=86_400 — 0 stays legal because it is the documented
///   "no periodic log line", and is the default; a day is the outer bound.
/// - `max_library_gb` 0..=10_000 — 0 stays legal because it is the documented
///   "no ceiling", and is the default; 10 TB is past any drive this is likely
///   to run on, and the ceiling only ever *deletes*, so an absurdly large
///   value is inert while an absurdly small one is already handled (favorites
///   are never pruned).
/// - `system_volume` 0..=200 — a percentage, where 100 is unity: the audio as
///   the system mixed it. Above unity Clippy amplifies, which can clip, and the
///   ceiling is [`clippy_core::capture::audio::MAX_VOLUME_PERCENT`] rather than a
///   number written here, so the bound and the gain curve cannot drift apart.
///   0 stays legal and means the loopback stream is never opened.
/// - `mic_volume` 0..=200 — the same taper, and 0 carries more weight here than
///   anywhere else in this table: it is the *only* way to turn the microphone
///   off, because there is no separate toggle. Windows shows a microphone
///   indicator whenever a process holds an input stream, so 0 must leave the
///   stream unopened rather than open it and scale to silence.
///
/// The keys deliberately absent are the non-numeric ones. `rate_control` and
/// `gpu_priority` both document falling back on an unknown value
/// (`Config::rate_control`, `Config::gpu_priority_low`) and that behaviour is
/// not this constant's to change; `clip_hotkey` is parsed by `clippy replay`'s
/// own loop; `clip_dir` is any path the user likes. What the table *must* not
/// do is fall silently behind `Config`, so
/// `every_numeric_config_key_is_range_checked` fails the build the day a
/// numeric key is added without a bound here.
const NUMERIC_BOUNDS: [(&str, u64, u64); 11] = [
    ("fps", MIN_FPS as u64, MAX_FPS as u64),
    ("bitrate_kbps", 0, 200_000),
    ("max_bitrate_kbps", 0, 200_000),
    ("replay_seconds", 1, 600),
    ("monitor_index", 0, 63),
    ("stats_seconds", 0, 86_400),
    ("max_library_gb", 0, 10_000),
    ("system_volume", 0, MAX_VOLUME_PERCENT as u64),
    ("mic_volume", 0, MAX_VOLUME_PERCENT as u64),
    ("auto_clip_min_kills", 1, 5),
    ("notification_volume", 0, 100),
];

/// Refuses the whole request if any bounded key is out of range.
///
/// Runs before anything is merged, deserialized, or written, so a rejection has
/// touched neither the daemon's in-memory config nor the file — `config.set` is
/// all-or-nothing across the whole `values` map, and a settings page that sent
/// five keys and got an error must not have to guess which two landed.
///
/// Only values that are already non-negative whole numbers are checked here.
/// A `-1`, a `1.5`, or a `"sixty"` is not a range problem but a *type* problem,
/// and [`Daemon::set_config`]'s existing per-key retry produces a far better
/// message for it ("does not accept …") than a range would. Skipping those here
/// lets each error say the true thing.
fn check_ranges(values: &Map<String, Value>) -> Result<()> {
    for (key, value) in values {
        let Some(&(_, min, max)) = NUMERIC_BOUNDS.iter().find(|(name, _, _)| name == key) else {
            continue;
        };
        let Some(number) = value.as_u64() else { continue };
        if number < min || number > max {
            bail!("{key:?} accepte une valeur de {min} à {max}, pas {number}");
        }
    }
    Ok(())
}

/// What `stats_seconds = 0` means to the control socket: one event a second.
///
/// 0 disables the periodic *log line* (`clippy replay`'s self-report) and that
/// stays true — but a client that sent `stats.subscribe {enabled:true}` has
/// asked to be told, and "off" is not an answer to that. 1 Hz is the rate spec
/// §4.4 describes and the one [`crate::clients::OUTBOUND_QUEUE_DEPTH`] is sized
/// against: fast enough for a live meter, slow enough to cost nothing
/// measurable next to a 60 fps hardware encode.
const DEFAULT_STATS_SECONDS: u32 = 1;

/// What `config.set` did (spec §4.3: "accepted values + which keys require a
/// re-arm to take effect").
///
/// `Debug`-derived so the brief's tests can call `expect_err` on a
/// `Result<ConfigUpdate>` — `expect_err` needs the `Ok` side printable for the
/// panic message it never actually reaches on a passing test.
#[derive(Debug)]
pub(crate) struct ConfigUpdate {
    /// The keys that were applied, with the values as they now stand — read
    /// back out of the saved config rather than echoed from the request, so a
    /// UI repaints from what the daemon holds and not from what it asked for.
    pub accepted: Map<String, Value>,
    /// Which of those keys need an `arm` before they change what is captured.
    pub requires_rearm: Vec<String>,
    /// `clip_dir` resolved the same way `config_json`'s `clip_dir_resolved`
    /// is. Carried out here, rather than left for `dispatch.rs` to ask
    /// `Daemon` for separately, because this module is deliberately
    /// transport-free (see the module doc) — `dispatch.rs` uses this to
    /// broadcast `config_changed` without a second round trip into `Daemon`.
    pub clip_dir_resolved: String,
}

/// Whether [`Daemon::record_saved_clip`] should play the configured clip
/// sound, which is the one behaviour a hotkey clip and a trim export do not
/// share.
///
/// A named enum rather than a `bool` because it is read at the call site, not
/// at the definition: `record_saved_clip(&meta, false)` says nothing about
/// what is being switched off, and this is exactly the parameter someone will
/// later add a third caller against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlaySound {
    /// A clip taken off the live ring, by hotkey, tray, or the `clip` command.
    /// The user may be mid-game with no Clippy window in front of them, and the
    /// chime is the only feedback there is.
    Yes,
    /// An export the user asked for in the window and is watching finish. The
    /// UI's own toast says so; the capture chime here would mean "the replay
    /// hotkey fired", which is not what happened.
    No,
}

pub struct Daemon {
    pub config: Mutex<Config>,
    /// Where `config.set` persists. `None` means there is nowhere to save —
    /// `%APPDATA%` unset — which is an error on that command and irrelevant to
    /// every other one, so it is not a startup failure.
    config_path: Option<PathBuf>,
    /// `Arc`, not a bare `Clients`, so `window::spawn` can hand a clone to the
    /// pump thread — see `window.rs`'s `CLIENTS` thread-local and its doc for
    /// why: `hotkey_pressed` has to reach `Clients::broadcast` from `wnd_proc`
    /// itself, unconditionally, or a press the daemon received while the
    /// action queue was full never gets reported at all.
    pub clients: Arc<Clients>,
    armed: Mutex<Option<Armed>>,
    /// The library, scanned once at startup and updated incrementally.
    /// `library.list` never touches the disk after that (spec §5.2).
    pub library: Mutex<Vec<ClipMeta>>,
    /// The library index and caches, outside the clips folder.
    store: Store,
    /// The capture levels, owned for the daemon's whole lifetime and cloned
    /// into each armed session.
    ///
    /// One instance, not one per arm: `config.set` has to be able to apply a
    /// level whether or not anything is armed, and building a fresh one per
    /// arm would mean it had to find whichever instance the live session
    /// happened to hold — the arrangement where a level change silently lands
    /// on an object nobody is reading.
    pub(crate) gains: Arc<AudioGains>,
    /// See [`GameWatch`].
    game: Mutex<GameWatch>,
    /// The games list. A leaf lock like `game`: nothing is taken while it is
    /// held, and events are broadcast after it is released.
    games: Mutex<Registry>,
    /// Serialises the per-track audio extraction, so two editors opening the
    /// same clip do not write the same files at once.
    track_files: Mutex<()>,
    pub(crate) export_jobs: crate::export_jobs::ExportJobs,
    pub(crate) export_active: Arc<std::sync::atomic::AtomicBool>,
}

// Lock ordering, and the one rule that matters: `armed` is never held while
// `clients` is locked. Arming, disarming, and clipping all take `armed`; every
// event that results from them is broadcast by `dispatch` *after* those methods
// have returned and released it. So a broadcast — which locks `clients` and
// nothing else — can never deadlock against an arm, however many clients are
// connected. `armed` may be held while `config` or `library` is taken (that is
// the order `status` and `clip` use); the reverse never happens.
//
// `armed` is deliberately held across the slow operations: `EngineHandle::spawn`
// can take seconds, `EngineHandle::clip` blocks until the mux completes, and
// `disarm` holds it until `EngineHandle::stop` has joined *and* the
// single-instance slot has been released. A concurrent `status` waits that out,
// which is the right trade — the alternative is two threads racing to spawn two
// engines, a `disarm` tearing the ring down in the middle of a clip's write, or
// a daemon that answers `armed: false` while it is still holding the slot that
// makes the next `arm` fail.

/// [`library::scan`] with the measurement around it.
///
/// The count and the elapsed time are logged at `info` deliberately rather than
/// `debug`: plan 3 owes a number for how a 5,000-clip library behaves, and this
/// is the instrument that produces it — from a real user's directory, on the
/// path that actually runs, without a benchmark harness that would measure
/// something else. `elapsed_ms` is an integer field rather than a formatted
/// `Duration` so the answer can be grepped straight out of the daemon log.
fn scan_and_log(dir: &Path, store: &Store) -> Result<Vec<ClipMeta>> {
    let started = std::time::Instant::now();
    let clips = clippy_core::store::scan_all(dir, store)?;
    tracing::info!(
        clips = clips.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        dir = %dir.display(),
        "scanned the clip library"
    );
    Ok(clips)
}

/// What a game's session records and where its clips go.
///
/// The game's own sound, alone: playing with Discord open does not put the
/// call in the clip unless the game's profile says so. The profile adds each
/// program as a track of its own and, when asked, the microphone.
fn game_session(seen: &Sighting, entry: Option<&GameEntry>) -> SessionContext {
    let name = entry.map(|e| e.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| seen.name.clone());
    let profile = entry.map(|e| e.profile.clone()).unwrap_or_else(GameProfile::default);
    let mut tracks = vec![TrackSpec { name: name.clone(), kind: TrackKind::Apps(vec![seen.exe.clone()]) }];
    for app in &profile.apps {
        tracks.push(TrackSpec {
            name: app.name.clone(),
            kind: TrackKind::Apps(vec![app.exe.clone()]),
        });
    }
    if profile.mic {
        tracks.push(TrackSpec { name: "Micro".to_string(), kind: TrackKind::Mic });
    }
    let folder = library::sanitize_folder(&name)
        .or_else(|| library::sanitize_folder(&crate::games::name_from_exe(&seen.exe)))
        .unwrap_or_else(|| library::DESKTOP_FOLDER.to_string());
    SessionContext { tracks: Some(tracks), game: Some(GameContext { name, folder }) }
}

/// Where a clip taken now is filed when the session itself does not say: in
/// desktop mode with `group_by_game`, the game in front (or still running)
/// gets its own folder, as it would in game mode. `None` keeps the session's.
fn place_for(group_by_game: bool, session_has_game: bool, seen: Option<&Sighting>, entry: Option<&GameEntry>) -> Option<clippy_core::engine::ClipPlace> {
    if !group_by_game || session_has_game {
        return None;
    }
    let game = game_session(seen?, entry).game?;
    Some(clippy_core::engine::ClipPlace { folder: game.folder, game: game.name })
}

/// Unknown modification times force a rebuild; cache bytes must describe
/// the current master rather than an older file with the same clip id.
fn track_cache_is_fresh(source: &Path, cached: &Path) -> bool {
    let Ok(source) = std::fs::metadata(source).and_then(|m| m.modified()) else { return false };
    let Ok(cached) = std::fs::metadata(cached) else { return false };
    cached.is_file()
        && cached.len() > 0
        && cached.modified().is_ok_and(|modified| modified >= source)
}

/// The track list of a remixed export: the sum first when several tracks were
/// kept, as a recording lays them out, then each by its name in the source.
fn exported_track_names(source_names: &[String], kept: &[export::TrackChoice]) -> Vec<String> {
    let mut names: Vec<String> = kept
        .iter()
        .map(|k| {
            source_names
                .get(k.ordinal)
                .cloned()
                .unwrap_or_else(|| format!("Piste {}", k.ordinal + 1))
        })
        .collect();
    if names.len() > 1 {
        names.insert(0, "Mix".to_string());
    }
    names
}

/// A config as a JSON object.
///
/// Every field, straight off the `Serialize` derive, so a config key added
/// later reaches `config.get` and becomes settable through `config.set` without
/// anyone remembering to touch this file. The `Result` is honoured rather than
/// unwrapped even though a struct of `u32`s and `String`s cannot fail to
/// serialize: this runs on a path a socket message reaches directly, in a
/// `panic = "abort"` build.
fn config_object(config: &Config) -> Result<Map<String, Value>> {
    match serde_json::to_value(config) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(other) => bail!("the config did not serialize as an object: {other}"),
        Err(e) => Err(e).context("serializing the config"),
    }
}

impl Daemon {
    /// The daemon over the real `%APPDATA%\clippy\config.toml`. Production
    /// callers want this one.
    pub fn new(config: Config) -> Self {
        Self::new_at(config, Config::path())
    }

    /// The daemon with `config.set` persisting to `config_path` — the same seam
    /// [`crate::pipe::serve_at`] is to `pipe::serve`, and for the same reason:
    /// without it, a test that exercises `config.set` at all would rewrite the
    /// settings of whoever ran `cargo test`.
    pub(crate) fn new_at(config: Config, config_path: Option<PathBuf>) -> Self {
        let clip_dir = config.clip_dir_path();
        // Best-effort, deliberately not fatal. On a fresh install
        // `%USERPROFILE%\Videos\Clippy` does not exist yet, and creating it here
        // is what stops the tray's "Open clips folder" launching Explorer at a
        // path that is not there. On a machine whose clips live on an external
        // drive the directory may be genuinely absent right now and back in a
        // minute — refusing to start over that would be absurd. [`Self::arm`]
        // is where an unusable clip directory becomes an error, because that is
        // where the user is present and can act on it.
        if let Err(e) = library::ensure_writable(&clip_dir) {
            tracing::warn!(
                dir = %clip_dir.display(),
                error = %format!("{e:#}"),
                "the clip directory is not usable yet; arming will refuse until it is"
            );
        }
        // The user's index under %APPDATA%; a test's beside its scratch config;
        // with no config at all, a private scratch one, so nothing reaches the
        // user's.
        let store = match &config_path {
            Some(path) if Config::path().as_ref() == Some(path) => Store::for_user().unwrap_or_else(|| Store::beside(path)),
            Some(path) => Store::beside(path),
            None => {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Store::beside(&std::env::temp_dir().join(format!("clippy-store-{}-{n}", std::process::id())).join("config.toml"))
            }
        };
        // One scan, at startup. A failure here is not fatal: an unreadable clip
        // directory must not stop the daemon from arming and capturing.
        let library = match scan_and_log(&clip_dir, &store) {
            Ok(clips) => clips,
            Err(e) => {
                tracing::warn!(
                    error = %format!("{e:#}"),
                    "could not scan the clip library at startup; starting empty"
                );
                Vec::new()
            }
        };
        let gains = AudioGains::new(config.system_volume, config.mic_volume);
        let mut registry = Registry::load(config_path.as_ref().map(|p| p.with_file_name("games.json")));
        // The old hand-kept list becomes entries of the games list, once.
        for exe in &config.game_apps {
            if crate::games::normalize_exe(exe).is_some_and(|exe| registry.find(&exe).is_none()) {
                let _ = registry.set(exe, Edit::default(), "");
            }
        }
        Self {
            config: Mutex::new(config),
            config_path,
            clients: Arc::new(Clients::default()),
            armed: Mutex::new(None),
            library: Mutex::new(library),
            store,
            gains,
            game: Mutex::new(GameWatch::default()),
            games: Mutex::new(registry),
            track_files: Mutex::new(()),
            export_jobs: crate::export_jobs::ExportJobs::default(),
            export_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Starts capture. In `game` mode that means "be ready": the replay waits for
    /// a game to be in front and starts by itself, then stops when the game ends,
    /// so nothing on the desktop is ever recorded. In `desktop` mode it starts
    /// at once. Idempotent either way.
    pub fn arm(&self) -> Result<ArmOutcome> {
        if self.lock_config().capture_mode() != CaptureMode::Game {
            return self.arm_now(None, SessionContext::default());
        }
        {
            let armed = self.lock_armed();
            if armed.is_some() {
                return Ok(ArmOutcome { status: self.status_of(armed.as_ref()) });
            }
        }
        let sighting = {
            let mut game = self.lock_game();
            game.wanted = true;
            game.sighting.clone()
        };
        match sighting {
            Some(seen) => self.start_for_game(&seen),
            None => {
                crate::window::publish_waiting(true);
                let status = self.status();
                self.clients.broadcast(&Event::new("standby", status.to_json()));
                Ok(ArmOutcome { status })
            }
        }
    }

    /// Starts the engine on the screen the game is on, and remembers that a game
    /// is why it runs, so the end of the game ends it.
    fn start_for_game(&self, seen: &Sighting) -> Result<ArmOutcome> {
        let screen = clippy_core::probe::monitors_true_pixels().ok().and_then(|found| {
            found.iter().find(|m| m.left == seen.left && m.top == seen.top).map(|m| m.index)
        });
        let entry = self.lock_games().find(&seen.exe).cloned();
        let outcome = self.arm_now(screen, game_session(seen, entry.as_ref()))?;
        self.lock_game().started_by_game = true;
        tracing::info!(game = %seen.exe, "a game is in front: the replay is recording");
        Ok(outcome)
    }

    /// One look at the game watcher's findings, once a second.
    ///
    /// `seen` is the game in front or still running, if any. A game that
    /// disappears for under [`GAME_GRACE`] (a loading screen swapping windows)
    /// is still the game. The replay starts when one is seen while it is wanted
    /// and idle, and stops when it was started for a game and none is left.
    /// Records what the watcher saw, in every capture mode: auto-clip needs
    /// to know the game in front even while the whole desktop is recorded.
    /// Starting and stopping the replay stays [`Self::game_tick`]'s, in the
    /// game mode only.
    /// See [`place_for`]. Takes `config`, `game` and `games` one after the
    /// other, never nested.
    fn clip_place(&self) -> Option<clippy_core::engine::ClipPlace> {
        let (group, session_has_game) = {
            let config = self.lock_config();
            (config.group_by_game, config.session.game.is_some())
        };
        if !group || session_has_game {
            return None;
        }
        // Only a game Clippy knows: a full-screen program it merely guessed at
        // would leave a folder named after a video player or a slideshow.
        let seen = self.lock_game().sighting.clone().filter(|seen| seen.known)?;
        let entry = self.lock_games().find(&seen.exe).cloned();
        place_for(group, session_has_game, Some(&seen), entry.as_ref())
    }

    pub fn note_sighting(&self, seen: Option<Sighting>, now: Instant) {
        Self::update_sighting(&mut self.lock_game(), seen, now);
    }

    fn update_sighting(game: &mut GameWatch, seen: Option<Sighting>, now: Instant) {
        match seen {
            Some(found) => {
                game.last_seen = Some(now);
                game.sighting = Some(found);
            }
            None => {
                if game.last_seen.is_none_or(|at| now.duration_since(at) >= GAME_GRACE) {
                    game.sighting = None;
                }
            }
        }
    }

    pub fn game_tick(&self, seen: Option<Sighting>, now: Instant) {
        let (start, stop) = {
            let mut game = self.lock_game();
            Self::update_sighting(&mut game, seen, now);
            let armed = self.lock_armed().is_some();
            let retry_ok = game.last_attempt.is_none_or(|at| now.duration_since(at) >= GAME_RETRY);
            let start =
                (!armed && game.wanted && retry_ok).then(|| game.sighting.clone()).flatten();
            if start.is_some() {
                game.last_attempt = Some(now);
            }
            (start, armed && game.started_by_game && game.sighting.is_none())
        };
        if let Some(seen) = start {
            if let Err(e) = self.start_for_game(&seen) {
                tracing::warn!(error = %format!("{e:#}"), "could not start the replay for a game; will retry");
            }
        } else if stop {
            tracing::info!("the game ended: the replay is waiting for the next one");
            self.lock_game().started_by_game = false;
            if let Err(e) = self.stop_engine("standby") {
                tracing::warn!(error = %format!("{e:#}"), "could not pause the replay");
            }
        }
    }

    pub fn capture_mode(&self) -> CaptureMode {
        self.lock_config().capture_mode()
    }

    /// What the detector must know about the user's own choices.
    pub fn game_lists(&self) -> Lists {
        let games = self.lock_games();
        Lists { games: games.manual_exes(), never: games.disabled_exes() }
    }

    /// The watcher saw a game running: adds it to the list on first sight and
    /// keeps its "last played" fresh. Tells clients when the list changed.
    pub fn game_seen(&self, seen: &Sighting) {
        // A full-screen program Clippy only guesses is a game is recorded in
        // the `game` mode but never listed.
        if !seen.known {
            return;
        }
        let now = Instant::now();
        {
            let mut watch = self.lock_game();
            let recent = watch.last_noted.as_ref().is_some_and(|(exe, at)| {
                *exe == seen.exe && now.duration_since(*at) < GAME_NOTE_EVERY
            });
            if recent {
                return;
            }
            watch.last_noted = Some((seen.exe.clone(), now));
        }
        let changed = self.lock_games().observe(&seen.exe, &seen.name, &library::now_rfc3339_local());
        if changed {
            self.broadcast_games();
        }
    }

    /// `games.list`.
    pub fn games_list(&self) -> Vec<GameEntry> {
        self.lock_games().list()
    }

    /// `games.set`: edits a game, or adds the program by hand.
    pub fn games_set(&self, exe: &str, edit: Edit) -> Result<GameEntry> {
        let fallback = crate::game::list_windows()
            .into_iter()
            .find(|w| w.exe.eq_ignore_ascii_case(exe.trim()))
            .map(|w| w.name)
            .unwrap_or_default();
        let entry = self.lock_games().set(exe, edit, &fallback).map_err(anyhow::Error::msg)?;
        self.broadcast_games();
        Ok(entry)
    }

    /// `games.remove`.
    pub fn games_remove(&self, exe: &str) -> Result<()> {
        if !self.lock_games().remove(exe) {
            bail!("« {exe} » n’est pas dans la liste des jeux");
        }
        self.broadcast_games();
        Ok(())
    }

    /// Adds the installed games found by the last scan that the list does not
    /// have yet. Returns how many were added.
    pub fn games_add_installed(&self) -> usize {
        let found: Vec<(String, String)> = crate::installed::current()
            .games()
            .into_iter()
            .filter_map(|(path, name)| {
                let exe = path.rsplit('\\').next()?.to_string();
                let name = if name.is_empty() { crate::game::friendly_name(&path, &exe) } else { name };
                Some((exe, name))
            })
            .collect();
        let added = self.lock_games().add_installed(&found);
        if added > 0 {
            self.broadcast_games();
        }
        added
    }

    /// `games.scan`: looks for installed games now and adds the new ones.
    /// Returns the list and how many were added.
    pub fn games_scan(&self) -> (Vec<GameEntry>, usize) {
        crate::installed::rescan();
        let added = self.games_add_installed();
        (self.games_list(), added)
    }

    fn broadcast_games(&self) {
        let games = self.games_list();
        let list = serde_json::to_value(&games).unwrap_or_else(|_| Value::Array(Vec::new()));
        let mut payload = Map::new();
        payload.insert("games".to_string(), list);
        self.clients.broadcast(&Event::new("games_changed", Value::Object(payload)));
    }

    fn lock_games(&self) -> MutexGuard<'_, Registry> {
        self.games.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reconciles a running or waiting replay with a `capture_mode` that just
    /// changed, so the setting takes effect now and not at the next arm.
    fn capture_mode_changed(&self, mode: CaptureMode) {
        let armed = self.lock_armed().is_some();
        match mode {
            CaptureMode::Desktop => {
                let waiting = {
                    let mut game = self.lock_game();
                    game.started_by_game = false;
                    game.wanted && !armed
                };
                if waiting && let Err(e) = self.arm_now(None, SessionContext::default()) {
                    tracing::warn!(error = %format!("{e:#}"), "could not start the replay for the desktop");
                }
            }
            CaptureMode::Game if armed => {
                let sighted = {
                    let mut game = self.lock_game();
                    game.wanted = true;
                    game.started_by_game = game.sighting.is_some();
                    game.sighting.is_some()
                };
                if !sighted && let Err(e) = self.stop_engine("standby") {
                    tracing::warn!(error = %format!("{e:#}"), "could not pause the replay");
                }
            }
            CaptureMode::Game => {}
        }
    }

    /// Stops capture and releases the single-instance slot. `Ok(false)` means
    /// nothing was running or waiting: a no-op rather than an error, so a UI
    /// syncing its toggle after a reconnect never sees a spurious failure.
    pub fn disarm(&self) -> Result<bool> {
        let was_waiting = {
            let mut game = self.lock_game();
            let was = game.wanted;
            game.wanted = false;
            game.started_by_game = false;
            was
        };
        crate::window::publish_waiting(false);
        let stopped = self.stop_engine("disarmed")?;
        if !stopped && was_waiting {
            self.clients.broadcast(&Event::new("disarmed", Value::Object(Map::new())));
            return Ok(true);
        }
        Ok(stopped)
    }

    /// Starts capture on `monitor`, or on the configured screen when `None`.
    /// Idempotent: arming an already-armed daemon reports the
    /// current state rather than erroring, because a UI reconnecting after its
    /// own restart has no way to know whether the daemon it just found is
    /// already armed.
    fn arm_now(&self, monitor: Option<u32>, session: SessionContext) -> Result<ArmOutcome> {
        let mut armed = self.lock_armed();
        if armed.is_some() {
            return Ok(ArmOutcome { status: self.status_of(armed.as_ref()) });
        }

        // Before the slot and before the engine. An unusable clip directory is
        // the one arm failure a user fixes in ten seconds — plug the drive back
        // in, or pick another folder — and arming is the moment they are
        // present and paying attention. The alternative is a green tray icon
        // over a replay ring whose clips can never be saved, discovered an hour
        // later at the exact moment they wanted one. `clip_dir` is read per
        // clip rather than baked into the engine, so this is the last check
        // that happens before the ring starts filling.
        //
        // Cheap enough to be unconditional: one directory create and one probe
        // file against an `arm` that takes seconds to bring a hardware encoder
        // up. Taking `config` under `armed` keeps the documented lock order.
        // Bare "cannot arm", not a second sentence naming the directory:
        // `ensure_writable` already names it and gives the OS's reason, and
        // repeating it produced "cannot arm: the clip directory D:\x is not
        // usable: could not create clip directory D:\x: …".
        library::ensure_writable(&self.lock_config().clip_dir_path())
            .context("impossible d’activer le replay")?;

        // The slot first, then the engine. With a `clippy replay` session holding
        // the slot, "another clippy capture session … is already running" is the
        // message the user can act on; taking the slot first is what makes it
        // win over the confusing encoder-in-use error the engine would
        // otherwise produce.
        let slot = control::acquire_single_instance()?;
        let mut config = self.lock_config().clone();
        if let Some(monitor) = monitor {
            config.monitor_index = monitor;
        }
        // What this session records and where it files its clips; see
        // `SessionContext`. Built by the caller (a game's profile) and never
        // saved.
        config.session = session;
        // `spawn` returns only once the first capture session is live, so a
        // hardware failure ("no hardware encoder on the capture adapter")
        // arrives here verbatim instead of being lost on the engine thread.
        // Deliberately unbounded by a timeout: an honest arm takes seconds, and
        // interrupting one needs a thread-teardown design this task does not
        // have. On the `?`, `slot` drops and the single-instance mutex is
        // released before the error goes back — a failed arm must not leave the
        // CLI locked out.
        let engine = EngineHandle::spawn(config, Arc::clone(&self.gains))?;
        *armed = Some(Armed { engine, _slot: slot });
        // Here rather than in the tray's own handler: `arm` is reached from the
        // tray menu, the control socket, and the desktop UI in stage 4, and an
        // icon that only tracked its own menu would show "idle" through a
        // UI-initiated recording. A no-op when there is no window.
        crate::window::publish_waiting(false);
        crate::window::publish_armed(true);
        let status = self.status_of(armed.as_ref());
        // Released before the broadcast below, so `clients` is never taken
        // while `armed` is held — the lock ordering at the top of this file.
        drop(armed);
        // Broadcast here rather than in `dispatch::arm`, for the reason
        // `record_saved_clip` gives: the tray menu calls this function
        // directly and never passes through `dispatch`, so an event left at
        // that one call site simply did not happen for the tray. A desktop UI
        // left open while the user armed from the tray went on showing "idle"
        // indefinitely — `armed`/`disarmed` are the only signal it gets, and
        // it does not poll. Emitting from the one function both paths go
        // through is what makes that structural.
        self.clients.broadcast(&Event::new("armed", status.to_json()));
        Ok(ArmOutcome { status })
    }

    /// Stops the engine and releases the single-instance slot, then tells
    /// clients with `event`: `disarmed` when the user turned the replay off,
    /// `standby` when it is only waiting for the next game.
    fn stop_engine(&self, event: &str) -> Result<bool> {
        // The guard is bound, not left as a temporary of a `let-else`: a
        // temporary is released at the end of its statement, which would leave
        // `armed` reading `None` for the whole of the teardown below — while
        // the engine is still joining and the single-instance slot is still
        // held. In that window `status` reports `armed: false` about a daemon
        // that is still finalizing, and an `arm` racing it fails with "another
        // clippy capture session (record or replay) is already running", pointing
        // the user at a CLI that is not running. Holding it means a concurrent
        // caller waits out the teardown and then gets a true answer.
        //
        // This does not touch the lock ordering: `clients` is still never taken
        // while `armed` is held. Nothing in here locks `clients` — the
        // `disarmed` event is broadcast at the end of this function, after the
        // guard is dropped — and `engine.stop()` is `clippy-core`, which has no
        // idea the registry exists.
        let mut armed = self.lock_armed();
        let Some(state) = armed.take() else { return Ok(false) };
        let Armed { engine, _slot } = state;
        // Whatever the rebuild loop finally returned is logged, not propagated.
        // By the time it is known, capture *is* stopped and the daemon *is*
        // idle; reporting that as a failed disarm would leave a UI's toggle
        // disagreeing with reality, which is the one outcome worth avoiding
        // here. The daemon log keeps the detail.
        if let Err(e) = engine.stop() {
            tracing::warn!(error = %format!("{e:#}"), "the engine reported an error on the way out");
        }
        // Only now, with the encoder actually released, is the slot handed
        // back — otherwise a `clippy replay` racing the disarm could win the slot
        // and then fail on a still-busy encoder. The `armed` guard goes last,
        // so no other caller can observe the daemon as idle until the slot it
        // was holding is genuinely free.
        drop(_slot);
        drop(armed);
        // After the slot is back, so the icon never says "idle" about a daemon
        // that is still holding the encoder. See the note in `arm`.
        crate::window::publish_armed(false);
        // Same reasoning as the `armed` broadcast in `arm`, and the same
        // bug without it: disarming from the tray left an open UI showing
        // "armed", and its own disarm button could not fix it — that call
        // returns `Ok(false)` for an already-idle daemon, which broadcasts
        // nothing, so the toggle stayed wrong until the app restarted.
        let payload = if event == "standby" {
            crate::window::publish_waiting(true);
            self.status().to_json()
        } else {
            Value::Object(Map::new())
        };
        self.clients.broadcast(&Event::new(event, payload));
        Ok(true)
    }

    /// Saves a clip from the live ring. `Ok(None)` means there is no footage
    /// buffered yet; the caller turns that into the CLI's wording.
    ///
    /// Broadcasts `clip_saved` here rather than at the call sites, because
    /// there are two of them and only one ever did it. `dispatch::clip` (the
    /// socket command) broadcast; `window::handle_action`'s `Action::Clip`
    /// (the hotkey and the tray) did not, so a clip taken with the hotkey was
    /// written to disk and no client was ever told. The desktop app's grid
    /// only learned about it on a reload — which is precisely the failure
    /// spec §10's stage-4 gate opens by forbidding: "clip appears in the grid
    /// within a second of the hotkey".
    ///
    /// `dispatch::clip`'s comment had predicted the hotkey path would need
    /// this and asked for it to live in one place. It stayed at that one call
    /// site instead, and the second caller arrived a plan later without it.
    /// Emitting from inside the function every caller must go through is what
    /// makes that structural rather than a rule to remember.
    pub fn clip(&self) -> Result<Option<ClipMeta>> {
        // Before `armed`: the lock order lets `armed` be held while `config`
        // is taken, never the reverse.
        let place = self.clip_place();
        let saved = {
            let armed = self.lock_armed();
            let Some(state) = armed.as_ref() else {
                bail!("le replay n’est pas activé — activez-le d’abord");
            };
            state.engine.clip_in(place)?
        };
        let saved = saved.map(|saved| self.file_saved_clip(saved));
        if let Some(meta) = &saved {
            // The one path the chime belongs to: something was captured off
            // the live ring, and the user may well be mid-game with no window
            // in front of them.
            self.record_saved_clip(meta, PlaySound::Yes);
        }
        Ok(saved)
    }

    /// Records a clip the engine just wrote in the library index, and its
    /// thumbnail in the cache. A record that will not write costs the clip's
    /// track names, not the clip: the next scan adopts the file.
    fn file_saved_clip(&self, saved: clippy_core::engine::SavedClip) -> ClipMeta {
        let mut meta = saved.meta;
        match self.store.insert(&meta, &saved.path) {
            Ok(id) => meta.id = id,
            Err(e) => tracing::warn!(clip = %meta.id, error = %format!("{e:#}"), "clip saved without a library record"),
        }
        if let Some(jpeg) = &saved.thumb {
            self.store.write_thumb(&meta.id, jpeg);
        }
        meta
    }

    /// Watches `region` of the live picture (None stops) and returns its
    /// newest read-back. `Ok(None)` while the replay is off or no read-back
    /// finished since the last call.
    pub fn probe(
        &self,
        region: Option<clippy_core::capture::probe::ProbeRegion>,
    ) -> Result<Option<clippy_core::capture::stage::StagedFrame>> {
        let armed = self.lock_armed();
        match armed.as_ref() {
            Some(state) => state.engine.probe(region),
            None => Ok(None),
        }
    }

    /// Writes a screenshot from the live capture session.
    ///
    /// Armed-only, and it says so. A screenshot comes off the session that is
    /// already running, which is what makes it nearly free and what makes it
    /// correct inside fullscreen games. The wording mirrors [`Self::clip`]'s
    /// because it is the same failure.
    pub fn screenshot(&self) -> Result<ShotMeta> {
        let meta = {
            let armed = self.lock_armed();
            let Some(state) = armed.as_ref() else {
                bail!("le replay n’est pas activé — activez-le pour prendre une capture d’écran");
            };
            state.engine.screenshot(Some(self.store.shots_dir()))?
        };
        // After the `armed` lock is released, mirroring `record_saved_clip`
        // below: Task 7's hotkey path calls this function directly, the same
        // way `window::handle_action` calls `Daemon::clip` directly, so the
        // broadcast has to live here rather than in the dispatcher or it goes
        // missing for exactly the path that matters most. See
        // `record_saved_clip`'s comment for the clip-side version of this
        // mistake.
        self.record_saved_shot(&meta);
        Ok(meta)
    }

    /// One page of screenshots, newest first.
    ///
    /// Unlike the clip library, screenshots are **not** cached in memory: they
    /// are scanned from disk on demand. A screenshot has no sidecar to read, so
    /// a scan is a directory listing plus a 4 KB header read per file, and a
    /// cache would buy nothing while adding a second thing to keep in sync.
    pub fn shots_list(&self, offset: usize, limit: usize) -> ShotPage {
        let dir = shot::shots_dir(&self.clip_dir());
        let all = shot::scan(&dir).unwrap_or_else(|e| {
            // An unreadable folder is an empty tab, not a broken app: taking
            // screenshots still works, only browsing them is affected.
            tracing::warn!(dir = %dir.display(), error = %format!("{e:#}"), "could not scan screenshots");
            Vec::new()
        });
        ShotPage {
            total: all.len(),
            shots: all.into_iter().skip(offset).take(limit).collect(),
            offset,
        }
    }

    pub fn shots_delete(&self, id: &str) -> Result<()> {
        shot::delete(&shot::shots_dir(&self.clip_dir()), id)?;
        let _ = std::fs::remove_file(self.store.shot_thumb(id));
        Ok(())
    }

    /// The library cache (`thumbs`, `tracks`, `shots`), for the window to
    /// read thumbnails from.
    pub fn cache_dir(&self) -> PathBuf {
        self.store.cache_dir().to_path_buf()
    }

    pub fn shots_reveal(&self, id: &str) -> Result<()> {
        let path = self.shot_image_path(id)?;
        if !path.exists() {
            bail!("capture d’écran {id} introuvable");
        }
        crate::reveal::in_explorer(&path).with_context(|| {
            format!("impossible d’afficher la capture d’écran {id} dans l’Explorateur")
        })
    }

    /// Puts an already-saved screenshot back on the clipboard.
    pub fn shots_copy(&self, id: &str) -> Result<()> {
        let path = self.shot_image_path(id)?;
        // `shots.copy` is reached from a dispatcher client thread, not the
        // capture thread every other WIC/MF caller runs on — see
        // `reveal.rs`'s `select` for the same distinction made for the shell.
        // This thread's COM apartment state is not something this code
        // controls, and it should not lean on `ensure_mf_started`'s
        // `MFStartup` call to have supplied one: that call only runs on
        // whichever thread reaches the `OnceLock` first, and every later
        // thread — including one that never calls `CoInitializeEx` itself —
        // gets the implicit-MTA behaviour once the process has an apartment at
        // all (see `thumb.rs`'s `encode_jpeg_sized` comment for that side of
        // it). Entering an apartment here defensively is cheap and correct
        // under all three `CoInitializeEx` outcomes regardless of which of
        // those states this thread is already in, so there is no reason to
        // depend on the process-wide side effect instead. Held across the
        // whole decode, not just the `CoCreateInstance` call, because the
        // guard's `CoUninitialize` on drop must not run while WIC objects
        // created under it are still alive.
        let _apartment = crate::com::Apartment::enter()?;
        let (bgra, width, height, stride) = clippy_core::thumb::decode_jpeg_bgra(&path)
            .with_context(|| format!("impossible de lire la capture d’écran {id}"))?;
        clippy_core::clipboard::copy_bgra(&bgra, width, height, stride)
    }

    /// The single place an id from the socket becomes a screenshot path.
    ///
    /// `shot::delete` validates its own id, but `reveal` and `copy` build a path
    /// before touching the file, so the whitelist has to run here too. Never
    /// reach a screenshot file any other way — the same seal `paths_for` gives
    /// the clip library.
    fn shot_image_path(&self, id: &str) -> Result<PathBuf> {
        if !library::is_valid_id(id) {
            bail!("{id:?} n’est pas un identifiant de capture d’écran");
        }
        Ok(shot::image_path(&shot::shots_dir(&self.clip_dir()), id))
    }

    /// The configured screenshot combination, for the message pump to register.
    pub fn screenshot_hotkey(&self) -> String {
        self.lock_config().screenshot_hotkey.clone()
    }

    /// Takes a clip that has just been written to disk into the live library
    /// and tells every connected client about it.
    ///
    /// Split out of [`Self::clip`] so it can be tested. `clip` itself needs an
    /// armed engine with a real capture ring, so nothing in the suite can
    /// reach the lines below through it -- which is exactly how a saved clip
    /// went three plans without an event for the hotkey path. This half needs
    /// only a `ClipMeta`, so the guarantee that a recorded clip is an
    /// announced clip is now something a test can hold.
    ///
    /// `sound` is the one thing the two callers disagree about, and it is why
    /// this takes a parameter rather than always chiming. Everything else here
    /// -- the cache insert, the ceiling, the `clip_saved` broadcast -- has to
    /// happen for an export exactly as it does for a hotkey clip, which is why
    /// `export_clip` routes through this function instead of doing those three
    /// things by hand. The chime does not: it is the sound that has meant one
    /// thing since the feature shipped, "the replay hotkey fired", and hearing
    /// it because a window in front of you finished a trim you asked for is
    /// both redundant and, mid-game, actively confusing.
    fn record_saved_clip(&self, meta: &ClipMeta, sound: PlaySound) {
        // Read before the library lock is taken, not after. `config` is only
        // ever held alone or under `armed` (see the lock ordering above), and
        // taking it after `library` here would introduce the first place in
        // the daemon where those two are held the other way round.
        //
        // Held once across both the toggle and the path lookup, rather than
        // two separate `lock_config` calls: `set_config` holds this same
        // lock across a merge, a Media Foundation decode, and a disk write,
        // and is reachable concurrently from a client dispatch thread and
        // the detached sound-dialog thread. Two reads here could straddle a
        // `config.set` between them and pair `clip_sound` from one config
        // generation with a path resolved from the next. `resolve_sound`
        // takes no lock of its own -- it only reads the guard already held
        // -- so this cannot deadlock the way calling `custom_sound_source`
        // while holding the guard would.
        // The banner follows the chime's rule (a hotkey or automatic clip,
        // never an export the window asked for), with its own toggle.
        let banner = matches!(sound, PlaySound::Yes) && self.lock_config().clip_banner;
        let sound = match sound {
            PlaySound::No => None,
            PlaySound::Yes => {
                let config = self.lock_config();
                // `None` here means "do not play"; `Some(None)` means the
                // built-in chime, which is what `resolve_sound` returns for a
                // sound that is unset or has nowhere cached. `play` takes
                // `None` to mean the built-in chime either way.
                config
                    .clip_sound
                    .then(|| (resolve_sound(&config, self.config_path.as_deref()), config.notification_volume))
            }
        };

        // Prepended, matching `library::scan`'s newest-first order. This is
        // the incremental update that keeps `library.list` off the disk
        // after startup (spec §5.2).
        self.lock_library().insert(0, meta.clone());
        let removed = self.enforce_library_ceiling();
        // After the insert, so a client that answers the event by calling
        // `library.list` cannot race ahead of the clip it was told about.
        match serde_json::to_value(meta) {
            Ok(mut data) => {
                if let Some(fields) = data.as_object_mut() {
                    let summary = self.list(0, 0);
                    fields.insert("library_total".into(), Value::from(summary.total));
                    fields.insert("library_bytes".into(), Value::from(summary.total_bytes));
                    fields.insert(
                        "removed_ids".into(),
                        Value::Array(removed.into_iter().map(Value::String).collect()),
                    );
                }
                self.clients.broadcast(&Event::new("clip_saved", data));
            }
            // `ClipMeta` is strings, integers and bools, so this cannot
            // actually happen -- but `panic = "abort"` leaves no room for
            // an `unwrap` on a path the hotkey reaches, and a clip that
            // saved is still saved even if nobody can be told.
            Err(e) => {
                tracing::error!(error = %e, "could not serialize a saved clip for clip_saved");
            }
        }

        // Last, after the clip is on disk and every client has been told. The
        // sound is feedback about a completed clip, so it goes behind
        // everything that makes the clip real.
        if let Some((custom, volume)) = sound {
            crate::sound::play(custom.as_deref(), volume);
        }
        if banner {
            crate::banner::show(clip_banner_message(meta));
        }
    }

    /// Tells every connected client that a screenshot has just been written,
    /// and plays the chime if configured to.
    ///
    /// Split out of [`Self::screenshot`] for the same reason
    /// [`Self::record_saved_clip`] is split out of [`Self::clip`]: the only
    /// thing this needs is a `ShotMeta`, so every caller — socket command
    /// today, hotkey in Task 7 — goes through the one place that announces a
    /// saved screenshot, rather than each caller having to remember to do it.
    /// Unlike clips, there is no in-memory cache to update and no ceiling to
    /// enforce here, because [`Self::shots_list`] scans the folder on demand.
    fn record_saved_shot(&self, meta: &ShotMeta) {
        // `config` is read fresh here rather than threaded down from
        // `screenshot`, which already reads it after the `armed` lock is
        // released — see the lock ordering note at the top of this file.
        let (play_sound, banner, volume) = {
            let config = self.lock_config();
            (config.screenshot_sound, config.clip_banner, config.notification_volume)
        };

        // Broadcast before the sound, matching `record_saved_clip`: the event
        // is what makes the screenshot real to a client, and the chime is
        // feedback about that, not the other way round.
        match serde_json::to_value(meta) {
            Ok(data) => self.clients.broadcast(&Event::new("shot_saved", data)),
            // `ShotMeta` is strings, integers and bools, so this cannot
            // actually happen -- but `panic = "abort"` leaves no room for an
            // `unwrap` on a path the hotkey reaches, and a screenshot that
            // saved is still saved even if nobody can be told.
            Err(e) => {
                tracing::error!(error = %e, "could not serialize a saved screenshot for shot_saved");
            }
        }

        if play_sound {
            crate::sound::play_shot(volume);
        }
        if banner {
            crate::banner::show(crate::banner::Message {
                title: "Capture d’écran enregistrée".into(),
                detail: "Copiée dans le presse-papiers".into(),
            });
        }
    }

    pub fn status(&self) -> DaemonStatus {
        let armed = self.lock_armed();
        self.status_of(armed.as_ref())
    }

    /// Takes the `armed` slot as an argument rather than locking it, so `arm`
    /// can build the outcome's status without dropping and retaking the lock —
    /// which would leave a window where the answer describes a different state
    /// than the one just entered.
    fn status_of(&self, armed: Option<&Armed>) -> DaemonStatus {
        let config = self.lock_config();
        let clip_dir = config.clip_dir_path().to_string_lossy().into_owned();
        let cache_dir = self.store.cache_dir().to_string_lossy().into_owned();
        // Read from the pump rather than carried on `Daemon`: the pump is what
        // binds them and the only thing that knows the outcome, and it is
        // forbidden from calling into `Daemon` to push it here (see
        // `window.rs`'s module comment). Both are `None` in a unit test and in
        // `clippy.exe`, neither of which starts a pump — which is the honest
        // answer for a process that never tried.
        let clip_hotkey_bound = crate::window::hotkey_bound(crate::window::HotkeyKind::Clip);
        let screenshot_hotkey_bound =
            crate::window::hotkey_bound(crate::window::HotkeyKind::Screenshot);
        let (wanted, game, game_name) = {
            let watch = self.lock_game();
            (
                watch.wanted,
                watch.sighting.as_ref().map(|s| s.exe.clone()),
                watch.sighting.as_ref().map(|s| s.name.clone()),
            )
        };
        match armed {
            // Armed: every field comes from the engine, so a `monitor_index`
            // changed in config since arming still reports what is *actually*
            // being captured.
            Some(state) => {
                let engine = state.engine.status();
                DaemonStatus {
                    armed: true,
                    encoder: match engine.encoder.is_empty() {
                        true => None,
                        false => Some(engine.encoder),
                    },
                    monitor_index: engine.monitor_index,
                    ring_seconds_used: engine.ring_seconds_used,
                    ring_seconds_total: engine.ring_seconds_total,
                    clip_dir,
                    cache_dir,
                    clip_hotkey_bound,
                    screenshot_hotkey_bound,
                    waiting_for_game: false,
                    game,
                    game_name,
                }
            }
            // Idle: the config is the only truth there is.
            //
            // Except the hotkeys, which are bound whether or not anything is
            // armed — clipping is what arming is *for*, so a dead hotkey is
            // worth reporting to a user who has not armed yet most of all.
            None => DaemonStatus {
                armed: false,
                encoder: None,
                monitor_index: config.monitor_index,
                ring_seconds_used: 0.0,
                ring_seconds_total: config.replay_seconds,
                clip_dir,
                cache_dir,
                clip_hotkey_bound,
                screenshot_hotkey_bound,
                waiting_for_game: wanted,
                game,
                game_name,
            },
        }
    }

    // --- Configuration ------------------------------------------------------

    /// `config.get`: the effective config as a JSON object, plus
    /// `clip_dir_resolved`.
    ///
    /// The config's own keys come from its `Serialize` derive rather than a
    /// hand-written list, so a key added to `Config` later appears here without
    /// this file changing — which is what the key-set test in `dispatch.rs`
    /// pins down.
    ///
    /// `clip_dir_resolved` is not a config key and is not settable: `clip_dir`
    /// is empty by default and means "wherever the default is", and a settings
    /// page cannot show a user where their clips actually land without the
    /// daemon resolving it for them.
    /// Where clips are written, resolved. For the tray's "Open clips folder".
    pub fn clip_dir(&self) -> std::path::PathBuf {
        self.lock_config().clip_dir_path()
    }

    /// The configured clip hotkey, for `window::spawn`.
    ///
    /// Deliberately this one key rather than a whole-`Config` snapshot: a
    /// snapshot invites callers to read the other keys long after they have
    /// gone stale, and `config.set` can change any of them at any time. A
    /// changed hotkey is rebound live by `set_config` calling
    /// `window::rebind_hotkey` — but only when a pump is running and posting
    /// to it succeeds. `rebind_hotkey`'s own doc comment is the honest
    /// version of this: with no pump (unit tests, and the tail of shutdown)
    /// or a failed `PostMessageW`, the rebind is skipped entirely and the
    /// running registration — like this copy — stays stale until the next
    /// restart, exactly the behavior this feature exists to remove in the
    /// case where it does reach a live pump.
    pub fn clip_hotkey(&self) -> String {
        self.lock_config().clip_hotkey.clone()
    }

    /// Which mechanism the hotkeys should be listened for through.
    ///
    /// Read at startup to tell `window::spawn` how to bind, and again on every
    /// `config.set` that touches it. Parsed rather than handed over as the raw
    /// string, so an unreadable value falls back to standard in one place
    /// (`Config::hotkey_mode`) instead of at each call site.
    pub fn hotkey_mode(&self) -> clippy_core::config::HotkeyMode {
        self.lock_config().hotkey_mode()
    }

    /// Whether the daemon should arm itself on startup without being asked.
    ///
    /// Read once, by `main`. Unlike `discord_presence` there is no loop to
    /// re-read it: turning it on mid-session must not arm anything, because a
    /// setting that armed on the spot would take the encoder from a user who
    /// was only saying what they want *next* time.
    pub fn auto_arm(&self) -> bool {
        self.lock_config().auto_arm
    }

    /// Whether Discord presence should be up right now.
    ///
    /// Read live on every poll rather than captured once at startup, which is
    /// the whole reason `presence.rs` is a loop: this is how a `config.set`
    /// toggle reaches the presence thread, and there is no other wiring
    /// between them.
    pub fn auto_clip_settings(&self) -> AutoClipSettings {
        let config = self.lock_config();
        AutoClipSettings {
            enabled: config.auto_clip,
            min_kills: config.auto_clip_min_kills.max(1),
            replay_seconds: config.replay_seconds,
        }
    }

    pub fn discord_presence_enabled(&self) -> bool {
        self.lock_config().discord_presence
    }

    /// The converted sound cache to play, if there is one.
    ///
    /// `None` covers two different situations a caller does not need to tell
    /// apart: `clip_sound_path` is empty, meaning the user is on the built-in
    /// chime, and `clip_sound_path` names a file but there is nowhere a
    /// converted copy could have been cached (`config_path` is `None` —
    /// `%APPDATA%` unset). Either way [`crate::sound::play`] is told the same
    /// thing, `None`, and plays the built-in chime.
    ///
    /// Locks `config` internally and hands back an owned path rather than the
    /// guard, following [`Self::clip_dir`] and [`Self::clip_hotkey`] above:
    /// `config` is only ever taken alone or under `armed` (see the lock
    /// ordering note near the top of this file), and a method that handed the
    /// guard itself out to callers crate-wide would let a future one hold it
    /// across a call that also wants `armed` or `clients`.
    pub(crate) fn custom_sound_source(&self) -> Option<PathBuf> {
        let config = self.lock_config();
        resolve_sound(&config, self.config_path.as_deref())
    }

    pub fn config_json(&self) -> Result<Value> {
        let config = self.lock_config();
        let mut object = config_object(&config)?;
        object.insert(
            "clip_dir_resolved".to_string(),
            Value::from(config.clip_dir_path().to_string_lossy().as_ref()),
        );
        // Spec §7.4's first run is "no config file", and `Config::load` never
        // writes one, so this is an observation rather than a flag somebody
        // has to remember to clear.
        //
        // Read from this daemon's own `config_path` rather than from
        // `Config::path()`. They are the same thing in production, and very
        // much not in a test: a static lookup would consult the developer's
        // real `%APPDATA%\clippy\config.toml` and answer `true` no matter what
        // this daemon was pointed at, which would make the test below pass
        // for the wrong reason and hide the case it exists to cover.
        object.insert(
            "config_file_exists".to_string(),
            Value::Bool(self.config_path.as_ref().is_some_and(|path| path.is_file())),
        );
        // Overridden, not read from the file: the registry is the source of
        // truth for autostart (spec §7.3). A user who removed the Run entry in
        // regedit or a startup manager has disabled autostart, and a settings
        // page still showing the toggle on would be telling them otherwise.
        // The key exists in `Config` so it round-trips through `config.set`
        // and survives in the file; what it holds is advisory.
        object.insert("autostart".to_string(), Value::Bool(crate::autostart::is_enabled()));
        Ok(Value::Object(object))
    }

    /// `config.set`: applies known keys, refuses unknown ones by name, writes
    /// the file, and reports which of the keys need a re-arm.
    ///
    /// Nothing is applied and nothing is written unless *every* key is
    /// acceptable — a settings page that sent five keys and got an error must
    /// not have to guess which two landed. "Acceptable" is three separate
    /// gates: the key is one `Config` has, its value is in range
    /// ([`NUMERIC_BOUNDS`]), and it deserializes at the right type. All three
    /// run before the file is opened.
    pub(crate) fn set_config(&self, values: &Map<String, Value>) -> Result<ConfigUpdate> {
        // The known-key set is read off a serialized `Config::default()`, not
        // listed here, for the same reason `config_json` serializes: a config
        // key added later is settable without anyone remembering this function
        // exists.
        let defaults = config_object(&Config::default())?;
        for key in values.keys() {
            if !defaults.contains_key(key) {
                let known: Vec<&str> = defaults.keys().map(String::as_str).collect();
                bail!("clé inconnue {key:?} (clés connues\u{a0}: {})", known.join(", "));
            }
        }
        // Before the lock, before the merge, before the file: an out-of-range
        // value has cost nothing but the check by the time it is refused.
        check_ranges(values)?;

        // Held across the write, deliberately. `config.set` is a
        // read-modify-write of the whole file: two clients setting different
        // keys at once would otherwise each save a file built from what they
        // read, and the loser's key would silently revert. The cost is that a
        // concurrent `status` waits out a small write to local disk; the
        // alternative is a settings page that sometimes does not stick. This
        // does not touch the lock ordering documented above — `config` is only
        // ever taken alone or under `armed`, never the other way around, and
        // nothing here locks `armed` or `clients`.
        let mut config = self.lock_config();
        let base = config_object(&config)?;
        let mut merged = base.clone();
        for (key, value) in values {
            merged.insert(key.clone(), value.clone());
        }

        if let Some(mode) = values.get("capture_mode").and_then(Value::as_str)
            && !matches!(mode.trim().to_ascii_lowercase().as_str(), "desktop" | "game")
        {
            bail!("capture_mode accepte « desktop » ou « game », pas {mode:?}");
        }
        if let Some(level) = values.get("video_quality").and_then(Value::as_str)
            && !matches!(
                level.trim().to_ascii_lowercase().as_str(),
                "low" | "medium" | "high" | "very_high" | "custom"
            )
        {
            bail!("video_quality accepte low, medium, high, very_high ou custom, pas {level:?}");
        }
        if let Some(corner) = values.get("banner_position").and_then(Value::as_str)
            && !matches!(corner.trim().to_ascii_lowercase().as_str(), "top_right" | "top_left")
        {
            bail!("banner_position accepte « top_right » ou « top_left », pas {corner:?}");
        }

        let mut updated: Config = match serde_json::from_value(Value::Object(merged)) {
            Ok(updated) => updated,
            // `from_value`'s message says what was wrong ("invalid type:
            // string, expected u32") but not *which* key, and a settings page
            // has to be able to point at the offending field. Re-applying one
            // key at a time finds it. This costs an extra deserialize per key
            // on a path that has already failed, and nothing at all otherwise.
            Err(e) => {
                for (key, value) in values {
                    let mut one = base.clone();
                    one.insert(key.clone(), value.clone());
                    if serde_json::from_value::<Config>(Value::Object(one)).is_err() {
                        bail!("{key:?} n’accepte pas {value}\u{a0}: {e}");
                    }
                }
                bail!("valeur invalide\u{a0}: {e}");
            }
        };
        // Names typed by a client reach the capture byte for byte; see the
        // function for what it fixes. The list `config.get` then reports is the
        // cleaned one, so the picker repaints from what is really matched.
        updated.audio_apps = clippy_core::config::normalize_audio_apps(&updated.audio_apps);

        // The last gate, and the only one that touches the filesystem. A
        // `clip_dir` naming somewhere that cannot take clips is refused here,
        // before the registry and before the file, so the all-or-nothing
        // contract holds: the caller is told no, the old directory is still in
        // force, and nothing was written. A settings page or the tray's folder
        // picker therefore cannot leave the user pointed at a folder that
        // silently swallows every clip.
        //
        // Guarded on the key being present, not run every time: this writes a
        // probe file, and five unrelated keys should not wake a possibly-absent
        // drive to save a bitrate. `updated` rather than the raw value, so an
        // empty string is checked as the default it resolves to.
        //
        // Not wrapped in a `config.set:` context like the errors above.
        // `ensure_writable` already names the directory and the OS's reason,
        // and this message is shown verbatim in the tray's folder-picker
        // message box, where "config.set" is jargon about a protocol the user
        // has never heard of.
        //
        // The comparison against the *old* config, taken while its guard is
        // still held, is what tells a real move from a `clip_dir` set to what
        // it already was. Both sides go through `clip_dir_path`, so clearing
        // the key back to "" counts as a move only when the default resolves
        // somewhere else. The rescan at the end of this function reads it.
        let mut clip_dir_moved = false;
        if values.contains_key("clip_dir") {
            clip_dir_moved = updated.clip_dir_path() != config.clip_dir_path();
            if clip_dir_moved && self.export_active.load(std::sync::atomic::Ordering::Acquire) {
                bail!("attendez la fin de l’export ou annulez-le avant de changer le dossier des clips");
            }
            library::ensure_writable(&updated.clip_dir_path())?;
        }

        // Decoded here, before anything is written, for the same all-or-nothing
        // reason as the gate above: a file that is not audio must leave the
        // user with the sound they already had. The bytes are held rather than
        // written because the config file has not been saved yet -- a new cache
        // beside an unchanged config is exactly the mismatch this ordering
        // exists to prevent.
        //
        // Guarded on the key being present, so five unrelated keys do not start
        // Media Foundation to save a bitrate.
        let converted = if values.contains_key("clip_sound_path") {
            let chosen = updated.clip_sound_path.trim();
            if chosen.is_empty() {
                // Reset: nothing to convert, and the cache must go.
                Some(None)
            } else {
                Some(Some(
                    clippy_core::sound::decode::to_wav(std::path::Path::new(chosen))
                        .context("ce fichier ne peut pas être utilisé comme son")?,
                ))
            }
        } else {
            None
        };

        // Disk first, then memory — the same order the clip library uses. A
        // failed write leaves the daemon agreeing with the file rather than
        // capturing at a setting no restart would reproduce.
        let path = self.config_path.as_deref().context(
            "aucun emplacement où enregistrer la configuration — APPDATA n’est pas défini",
        )?;

        // Before the file, not after. `autostart` lives in the registry
        // (spec §7.3), so this is the one key whose real write is not
        // `save_to`, and it can fail on its own. Doing it first keeps this
        // function's all-or-nothing contract: a registry failure here means
        // nothing was written and nothing was applied, exactly like a value
        // that failed validation. Writing the file first would instead leave
        // every *other* key in this call persisted to disk while the caller
        // was told the call failed — and live after the next restart.
        //
        // The residue if the file write below fails is the registry entry
        // alone, which is harmless: the registry is the source of truth, so
        // `config.get` reports what actually happened rather than a stale
        // file value.
        if values.contains_key("autostart") {
            crate::autostart::set_enabled(updated.autostart).with_context(|| {
                if updated.autostart {
                    "impossible d’activer le lancement au démarrage de Windows"
                } else {
                    "impossible de désactiver le lancement au démarrage de Windows"
                }
            })?;
        }

        updated.save_to(path).context("impossible d’enregistrer la configuration")?;

        // After the config is on disk, not before. A failed write here
        // (the rename in `write_sound_cache` is atomic, so it can only
        // leave the *previous* sound's cache standing, never a
        // half-written one) would once have been trusted forever:
        // `repair_sound_cache`'s old guard was only "does a cache file
        // exist," with no record of which sound produced it. The sidecar
        // written beside a fresh cache -- and removed beside a deleted one,
        // below -- is what fixed that: `needs_sound_cache_rebuild` now
        // rebuilds whenever the cache is missing, its sidecar is missing, or
        // the sidecar names something other than the configured sound, so a
        // stale pair left here is still caught at the very next startup even
        // if nothing below ran at all. Deleting the stale cache and its
        // sidecar here is not what keeps that invariant true -- the sidecar
        // is -- it only means the daemon stops playing the old sound now,
        // rather than waiting for a restart to find out.
        match converted {
            Some(Some(wav)) => {
                if let Err(e) = write_sound_cache(path, &wav, updated.clip_sound_path.trim()) {
                    tracing::warn!(error = %format!("{e:#}"), "could not save the converted sound");
                    // Checked and logged, not silent: this has a *correlated*
                    // trigger, not just an independent one. `PlaySoundW`
                    // holding `clip-sound.wav` open for a Test click or a
                    // clip saving while a new sound is being picked is
                    // exactly what makes the rename above fail with a
                    // sharing violation -- and this delete, targeting the
                    // same open file, then fails for the same reason. Left
                    // unchecked, that leaves config naming the new sound,
                    // the cache still holding the old one, and nothing in
                    // the log to explain why -- until the next startup's
                    // `repair_sound_cache` catches it via the sidecar below.
                    if let Err(e) = std::fs::remove_file(Config::sound_cache_path(path)) {
                        tracing::warn!(
                            error = %format!("{e:#}"),
                            "could not remove the stale sound cache; the previous sound will \
                             keep playing until the daemon restarts"
                        );
                    }
                    // Paired with the cache above, unconditionally: its job
                    // was to vouch for a cache this attempt just failed to
                    // produce, so it has nothing left to vouch for. Harmless
                    // either way the delete above went -- if it also failed,
                    // the previous cache and this sidecar still name the
                    // same (old) sound and agree with each other, but
                    // `clip_sound_path` in the config just saved above
                    // already names the *new* one, so the mismatch that
                    // sends the next startup's `repair_sound_cache` down the
                    // rebuild path is already there regardless of whether
                    // this file survives.
                    let _ = std::fs::remove_file(Config::sound_src_path(path));
                }
            }
            Some(None) => {
                // Best-effort: there is usually no cache to remove, because the
                // user was on the built-in sound already. Paired with the
                // sidecar so the two never disagree about a sound that was
                // just reset away.
                let _ = std::fs::remove_file(Config::sound_cache_path(path));
                let _ = std::fs::remove_file(Config::sound_src_path(path));
            }
            None => {}
        }

        let after = config_object(&updated)?;
        let mut accepted = Map::new();
        let mut requires_rearm = Vec::new();
        for key in values.keys() {
            if let Some(value) = after.get(key) {
                accepted.insert(key.clone(), value.clone());
            }
            if REQUIRES_REARM.contains(&key.as_str())
                || crosses_zero(key, base.get(key), after.get(key))
            {
                requires_rearm.push(key.clone());
            }
        }
        *config = updated;
        crate::banner::set_corner(config.banner_corner());
        // The tray menu ticks and labels itself from this mirror.
        crate::window::publish_prefs(&config);

        // After the write, like the hotkey rebind below and for the same
        // reason: a live session capturing at a level the user was told had
        // failed to save is exactly what this ordering prevents.
        self.gains.set(config.system_volume, config.mic_volume);

        // After the write, not before: a rebind that beat a failed write would
        // leave the running hotkey and the saved hotkey disagreeing, and
        // `config.set` is all-or-nothing everywhere else.
        //
        // The mode is checked first and takes the whole branch, because
        // switching it re-binds *both* combinations by itself — running the
        // per-key rebinds underneath it would repeat work that was just done,
        // for the same specs, with the only difference being how many times
        // the pump is woken to do it.
        if values.contains_key("hotkey_mode") {
            crate::window::set_hotkey_mode(
                config.hotkey_mode(),
                &config.clip_hotkey,
                &config.screenshot_hotkey,
            );
        } else {
            if values.contains_key("clip_hotkey") {
                crate::window::rebind_hotkey(crate::window::HotkeyKind::Clip, &config.clip_hotkey);
            }
            // Same reasoning, same ordering, for the screenshot key: this is
            // what makes the `requires_rearm: []` computed above for
            // `screenshot_hotkey` an honest answer instead of one that is only
            // true after a restart.
            if values.contains_key("screenshot_hotkey") {
                crate::window::rebind_hotkey(
                    crate::window::HotkeyKind::Screenshot,
                    &config.screenshot_hotkey,
                );
            }
        }

        // Read under the same lock the write above just landed into, the same
        // way `config_json` computes it fresh rather than trusting a value
        // carried in from before the merge — `dispatch.rs` broadcasts this in
        // `config_changed` unconditionally, not only when `clip_dir` was one
        // of `values`, so a client always learns where clips land now.
        let clip_dir_resolved = config.clip_dir_path().to_string_lossy().into_owned();

        // The library cache was scanned against the *old* directory, and every
        // mutation since has kept it current for that one. A move invalidates
        // all of it at once: without this, `library.list` keeps answering with
        // clips that are no longer under the configured folder, a UI keeps
        // building `asset:` URLs that now resolve nowhere, and
        // `enforce_library_limit` prunes against ids whose files it can no
        // longer find.
        //
        // The guard is dropped first because `rescan_library` locks `config`
        // itself and this is a non-reentrant `Mutex`. Nothing below reads
        // `config` again -- `clip_dir_resolved` was taken from it on the line
        // above precisely so this drop is safe.
        //
        // A failed scan is a warning, not an error. The setting is written and
        // applied by now; answering `config.set` with a failure would tell the
        // user their folder change did not take when it did.
        let new_mode = config.capture_mode();
        drop(config);
        if clip_dir_moved {
            if let Err(e) = self.rescan_library() {
                tracing::warn!(
                    error = %format!("{e:#}"),
                    dir = %clip_dir_resolved,
                    "the new clips folder could not be scanned; the library list is stale"
                );
            }
        }

        // After the config guard is released: switching the mode may start or
        // stop the engine, and starting reads the config again.
        if values.contains_key("capture_mode") {
            self.capture_mode_changed(new_mode);
        }

        Ok(ConfigUpdate { accepted, requires_rearm, clip_dir_resolved })
    }

    /// Rebuilds the converted sound if it does not match the configured one.
    ///
    /// Three things reach this state and none of them self-heal on their own:
    /// a `config.toml` edited by hand to name a different sound than the one
    /// the cache was built from, a cache (or its sidecar) someone deleted
    /// while tidying `%APPDATA%`, and a process that died between saving the
    /// config and writing the cache. [`needs_sound_cache_rebuild`] is the
    /// guard that catches all three -- see its own doc comment.
    ///
    /// Called at startup rather than from the constructor so no test builds a
    /// `Daemon` that starts Media Foundation.
    pub fn repair_sound_cache(&self) {
        let Some(config_path) = self.config_path.as_deref() else {
            return;
        };
        let chosen = self.lock_config().clip_sound_path.trim().to_string();
        let cache_exists = Config::sound_cache_path(config_path).exists();
        // A missing sidecar reads the same as one that disagrees -- both are
        // "cannot vouch for this cache" -- so a read failure (not there, not
        // valid UTF-8, whatever) collapses to `None` rather than being
        // threaded through as its own case.
        let sidecar = std::fs::read_to_string(Config::sound_src_path(config_path)).ok();
        if !needs_sound_cache_rebuild(&chosen, cache_exists, sidecar.as_deref()) {
            return;
        }

        match clippy_core::sound::decode::to_wav(std::path::Path::new(&chosen)) {
            Ok(wav) => {
                if let Err(e) = write_sound_cache(config_path, &wav, &chosen) {
                    tracing::warn!(error = %format!("{e:#}"), "could not rebuild the clip sound");
                }
            }
            // The setting is deliberately left alone. Putting the file back and
            // restarting is then all it takes; clearing it here would quietly
            // discard a choice the user still wants.
            Err(e) => tracing::warn!(
                error = %format!("{e:#}"),
                sound = %chosen,
                "the chosen clip sound could not be read; using the built-in one"
            ),
        }
    }

    // --- Statistics ---------------------------------------------------------

    /// The live engine's counters, or `None` when nothing is armed.
    ///
    /// Private: [`Self::stats_json`] two methods below is the only caller, and
    /// a second one would be a second place that has to know `EngineStatus`
    /// only exists while armed.
    fn engine_status(&self) -> Option<EngineStatus> {
        self.lock_armed().as_ref().map(|state| state.engine.status())
    }

    /// How long the stats thread waits between `stats` events.
    pub fn stats_interval(&self) -> Duration {
        let seconds = self.lock_config().stats_seconds.max(DEFAULT_STATS_SECONDS);
        Duration::from_secs(u64::from(seconds))
    }

    /// The `stats` event's payload (spec §4.4), or `None` when nothing is
    /// armed — every counter in it comes from the live engine, and there is no
    /// honest value for "frames captured" without one.
    ///
    /// Built key by key with `Value::from` for the same reason
    /// [`DaemonStatus::to_json`] is: `serde_json::json!` hides an `unwrap` and
    /// this reaches the socket in a `panic = "abort"` build.
    ///
    /// No GPU memory numbers, deliberately. `StatsReporter` reads those off the
    /// capture session's D3D device; the daemon has no device even while armed,
    /// because the reporter lives *inside* the session. Standing a second DXGI
    /// device up in the daemon purely so it could report on itself would cost
    /// the memory this project exists to save. `clippy_core::stats::working_set`
    /// needs no device and is what ships.
    pub fn stats_json(&self) -> Option<Value> {
        let engine = self.engine_status()?;
        let mut fields = Map::new();
        fields.insert(
            "encoder".to_string(),
            match engine.encoder.is_empty() {
                true => Value::Null,
                false => Value::from(engine.encoder.as_str()),
            },
        );
        fields.insert("monitor_index".to_string(), Value::from(engine.monitor_index));
        fields.insert("width".to_string(), Value::from(engine.width));
        fields.insert("height".to_string(), Value::from(engine.height));
        fields.insert("fps".to_string(), Value::from(engine.fps));
        fields.insert("ring_seconds_used".to_string(), Value::from(engine.ring_seconds_used));
        fields.insert("ring_seconds_total".to_string(), Value::from(engine.ring_seconds_total));
        fields.insert("frames".to_string(), Value::from(engine.frames));
        fields.insert("dropped".to_string(), Value::from(engine.dropped));
        fields.insert("paced".to_string(), Value::from(engine.paced));
        fields.insert("working_set".to_string(), Value::from(stats::working_set()));
        Some(Value::Object(fields))
    }

    // --- The clip library ---------------------------------------------------
    //
    // The cache in `self.library` *is* the library as far as the socket is
    // concerned (spec §5.2): `list` never touches the disk, and every mutation
    // writes the disk first and then updates the cache, so a failed write never
    // leaves a client believing something happened that did not.

    /// Deletes the oldest non-favorite clips until the library fits under
    /// `max_library_gb`, and evicts whatever went from the cache (spec §5.4).
    ///
    /// Runs *after* a save, never before: the clip the user just asked for is
    /// never the one deleted to make room for itself. That means the library
    /// can sit one clip over the ceiling until the next save, which is the
    /// right trade — a ceiling is a housekeeping convenience, and losing the
    /// clip you just pressed the hotkey for is the product failing at its job.
    ///
    /// A failed prune never fails the clip that triggered it. The clip is on
    /// disk and in the cache by the time this runs; reporting an error now
    /// would tell the user their clip did not save when it did.
    ///
    /// The cache eviction is not optional bookkeeping. `library.list` reads the
    /// cache and never the disk, so a pruned clip left in it would be a row
    /// whose file cannot be opened, that `total` still counts, and that
    /// `rename` would happily write an orphan sidecar for — the same ghost-row
    /// failure [`Daemon::delete`] evicts to prevent.
    fn enforce_library_ceiling(&self) -> Vec<String> {
        let (dir, max_gb) = {
            let config = self.lock_config();
            (config.clip_dir_path(), config.max_library_gb)
        };
        match clippy_core::store::prune_to_ceiling(&dir, &self.store, max_gb) {
            Ok(removed) if removed.is_empty() => removed,
            Ok(removed) => {
                self.lock_library().retain(|clip| !removed.contains(&clip.id));
                tracing::info!(count = removed.len(), "the clip library ceiling pruned old clips");
                removed
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "the clip library ceiling failed");
                Vec::new()
            }
        }
    }

    /// Re-reads the clip directory and replaces the cache.
    ///
    /// Still wired to no command: [`Daemon::new`] scans at startup and every
    /// mutation below keeps the cache current, which is the whole point of
    /// caching it. Three things need the seam anyway — [`Daemon::set_config`],
    /// where a `clip_dir` move invalidates the whole cache at once and there is
    /// nothing incremental to do about it; a test that wants a known library
    /// without depending on what the startup scan happened to see; and the
    /// `library.refresh` a later plan will want for "I deleted clips in
    /// Explorer behind your back", which is then a one-line dispatch arm rather
    /// than a new code path.
    pub fn rescan_library(&self) -> Result<()> {
        let dir = self.lock_config().clip_dir_path();
        let clips = scan_and_log(&dir, &self.store)?;
        *self.lock_library() = clips;
        Ok(())
    }

    /// One page of the library, newest first — straight out of the cache, no
    /// disk access (spec §5.2).
    ///
    /// `limit` is already clamped to `MAX_LIST_LIMIT` by `Command::parse`, so
    /// it is not re-clamped here. `offset` is not, and does not need to be:
    /// `skip` past the end yields an empty iterator rather than the panic a
    /// slice range would give. An offset past the end is a short page, not an
    /// error — a UI paging a library that shrank under it should see the end of
    /// the list, not a failure.
    pub fn list(&self, offset: usize, limit: usize) -> LibraryPage {
        let library = self.lock_library();
        // Compute the summary under the same lock as the page. A client can
        // display the full size immediately and restart paging if a save,
        // prune, delete, or metadata edit moved the rows between requests.
        // Hashing cached metadata does not open any video or thumbnail files.
        let mut snapshot = DefaultHasher::new();
        let mut total_bytes = 0_u64;
        for clip in library.iter() {
            clip.id.hash(&mut snapshot);
            clip.bytes.hash(&mut snapshot);
            clip.title.hash(&mut snapshot);
            clip.favorite.hash(&mut snapshot);
            total_bytes = total_bytes.saturating_add(clip.bytes);
        }
        LibraryPage {
            total: library.len(),
            clips: library.iter().skip(offset).take(limit).cloned().collect(),
            offset,
            total_bytes,
            snapshot: format!("{:016x}", snapshot.finish()),
        }
    }

    /// Removes a clip's `.mp4`, its record and its cached thumbnail and audio,
    /// then drops it from the list.
    ///
    /// The `.mp4` is the clip: if it is not there, there is nothing to delete
    /// and that is an error — but the record and the row still go, since no
    /// client could ever name this id again otherwise (the user deleting the
    /// file in Explorer is exactly how that arises).
    pub fn delete(&self, id: &str) -> Result<()> {
        let paths = self.paths_for(id)?;
        let mp4_was_missing = match std::fs::remove_file(paths.mp4()) {
            Ok(()) => false,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                return Err(e).with_context(|| format!("impossible de supprimer le clip {id}"));
            }
        };
        {
            let _tracks = self.track_files.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(e) = self.store.forget(id) {
                tracing::warn!(clip = %id, error = %format!("{e:#}"), "the clip was deleted but its record was not");
            }
        }
        self.lock_library().retain(|clip| clip.id != id);
        if mp4_was_missing {
            bail!("clip {id} introuvable dans la bibliothèque")
        }
        Ok(())
    }

    /// Renames a clip: its title is its file name, so the `.mp4` is renamed
    /// in its folder. The id does not change, so drafts and selections keyed
    /// on it survive. Refused when another clip already has the name.
    pub fn rename(&self, id: &str, title: &str) -> Result<ClipMeta> {
        let title = title.trim();
        if title.is_empty() {
            bail!("le titre d’un clip ne peut pas être vide");
        }
        let _ = self.paths_for(id)?;
        let stem = self.store.rename(id, title)?;
        self.edit_listed(id, |meta| meta.title = stem)
    }

    /// The star, kept in the library index.
    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<ClipMeta> {
        let _ = self.paths_for(id)?;
        self.store
            .set_favorite(id, favorite)
            .with_context(|| format!("impossible de mettre à jour le clip {id}"))?;
        self.edit_listed(id, |meta| meta.favorite = favorite)
    }

    /// Shows the clip in Explorer, reusing a window that is already open on
    /// the clip folder instead of adding another one beside it.
    ///
    /// A missing `.mp4` is an error here and, unlike [`Daemon::delete`], does
    /// *not* evict the stale cache row. The asymmetry is deliberate: `delete`
    /// is a mutation the user asked for and its whole job is to make the row go
    /// away, so discovering the file was already gone is just an earlier route
    /// to the same outcome. `reveal` is read-shaped — a UI sends it on a click,
    /// sometimes on hover — and a read that silently deletes the row the user
    /// was pointing at would surprise far more than the stale row does. The
    /// stale row still has a route out: the `delete` the user reaches for next,
    /// or the `library.refresh` a later plan owns.
    ///
    /// The Explorer work is [`crate::reveal::in_explorer`]. What stays here is
    /// the part that must not move: the id goes through [`Self::paths_for`], so
    /// no path handed to the shell can have come from an unvalidated id.
    pub fn reveal(&self, id: &str) -> Result<()> {
        let paths = self.paths_for(id)?;
        if !paths.mp4().exists() {
            bail!("clip {id} introuvable dans la bibliothèque");
        }
        crate::reveal::in_explorer(paths.mp4())
            .with_context(|| format!("impossible d’afficher le clip {id} dans l’Explorateur"))
    }

    /// Puts the clip's `.mp4` on the clipboard as a file, for a paste into
    /// Discord or any upload box. The path goes through [`Self::paths_for`]
    /// like every other clip path.
    pub fn copy_clip(&self, id: &str) -> Result<()> {
        let paths = self.paths_for(id)?;
        if !paths.mp4().is_file() {
            bail!("clip {id} introuvable dans la bibliothèque");
        }
        let path = std::path::absolute(paths.mp4())?;
        clippy_core::clipboard::copy_files(&[&path])
            .with_context(|| format!("impossible de copier le clip {id}"))
    }

    /// Every keyframe position in a clip, for the trim bar's ticks.
    ///
    /// [`Self::paths_for`] is the sealed choke point: it runs
    /// `library::is_valid_id` before it builds a single path, so a hostile id
    /// is refused here with no I/O. Never reach a clip file any other way —
    /// see the `mod clip_paths` doc comment for why that seal exists.
    ///
    /// Answers with the whole index rather than a page of it. A replay-buffer
    /// clip pinned to one keyframe per second yields tens of numbers, so
    /// paging would cost a protocol and buy nothing.
    pub(crate) fn keyframes(&self, clip_id: &str) -> Result<Value> {
        let paths = self.paths_for(clip_id)?;
        if !paths.mp4().is_file() {
            bail!("clip {clip_id} introuvable");
        }
        let keys = export::keyframes_ms(paths.mp4())
            .with_context(|| format!("impossible d’indexer les images clés du clip {clip_id}"))?;
        let mut out = Map::new();
        out.insert("clip_id".to_string(), Value::from(clip_id));
        out.insert("keyframes".to_string(), Value::from(keys));
        Ok(Value::Object(out))
    }

    /// Indexes actual video presentation boundaries through the sealed clip path.
    pub(crate) fn frames(&self, clip_id: &str) -> Result<Value> {
        let paths = self.paths_for(clip_id)?;
        if !paths.mp4().is_file() {
            bail!("clip {clip_id} introuvable");
        }
        let frames = export::frame_index(paths.mp4())
            .with_context(|| format!("impossible d’indexer les images du clip {clip_id}"))?;
        Ok(serde_json::json!({
            "timestamps_100ns": frames.timestamps_100ns,
            "end_100ns": frames.end_100ns,
        }))
    }


    /// Exports a range of a clip as a new clip in the library.
    ///
    /// Legacy synchronous entry point. The editor uses the daemon-owned job
    /// entry point for progress, cancellation and reconnection recovery.
    ///
    /// The source is only ever read. Everything written goes to an id this
    /// function allocates — there is no caller-supplied destination, so no
    /// request can aim an export at a path of its own choosing, and
    /// `export_fast` refuses a destination that names the source anyway.
    pub(crate) fn export_clip(
        &self,
        clip_id: &str,
        start_ms: u64,
        end_ms: u64,
        mode: &str,
        output: clippy_proto::command::ExportOutput,
        tracks: Option<&[clippy_proto::command::ExportTrack]>,
        shaping: Shaping,
    ) -> Result<ClipMeta> {
        let _permit = self.acquire_export_permit()?;
        self.export_clip_with_control(clip_id, start_ms, end_ms, mode, output, tracks, shaping, &export::ExportControl::default())
    }

    pub(crate) fn validate_export_request(&self, clip_id: &str, start_ms: u64, end_ms: u64, mode: &str, output: clippy_proto::command::ExportOutput, shaping: Shaping) -> Result<()> {
        Self::validate_shaping(mode, output, shaping)?;
        if !matches!(mode, "fast" | "precise") { bail!("mode d’export inconnu : \"{mode}\" ; choisissez \"fast\" ou \"precise\""); }
        let paths = self.paths_for(clip_id)?;
        if !paths.mp4().is_file() { bail!("clip {clip_id} introuvable"); }
        let meta = self.source_meta(clip_id, paths.mp4())?;
        Self::export_range(meta.duration_ms, start_ms, end_ms, mode)?;
        Ok(())
    }

    /// A size cap needs the reencode (`precise`) and a single mix (`share`):
    /// stems would spend the budget on audio nobody hears in a chat embed.
    /// A portrait crop needs the reencode too.
    fn validate_shaping(mode: &str, output: clippy_proto::command::ExportOutput, shaping: Shaping) -> Result<()> {
        if shaping.size_limit_mb.is_some() && (mode != "precise" || output != clippy_proto::command::ExportOutput::Share) {
            bail!("une taille maximale demande la coupe précise et le MP4 de partage");
        }
        if shaping.vertical_center.is_some() && mode != "precise" {
            bail!("le format vertical demande la coupe précise");
        }
        Ok(())
    }

    fn export_range(duration: u64, start: u64, end: u64, mode: &str) -> Result<(u64, u64)> {
        if mode != "precise" {
            return export::clamp_range(duration, start, end).map_err(anyhow::Error::msg);
        }
        let end = end.min(duration);
        if duration == 0 || start >= end { bail!("la plage précise doit contenir au moins une image"); }
        Ok((start, end))
    }

    pub(crate) fn export_clip_with_control(
        &self, clip_id: &str, start_ms: u64, end_ms: u64, mode: &str,
        output: clippy_proto::command::ExportOutput,
        tracks: Option<&[clippy_proto::command::ExportTrack]>,
        shaping: Shaping,
        control: &export::ExportControl,
    ) -> Result<ClipMeta> {
        Self::validate_shaping(mode, output, shaping)?;
        if !matches!(mode, "fast" | "precise") {
            bail!("mode d’export inconnu : \"{mode}\" ; choisissez \"fast\" ou \"precise\"");
        }
        let paths = self.paths_for(clip_id)?;
        if !paths.mp4().is_file() {
            bail!("clip {clip_id} introuvable");
        }
        // The source's record supplies dimensions, nominal fps and the
        // duration against which the requested range is validated.
        let source_meta = self.source_meta(clip_id, paths.mp4())?;
        let (start_ms, end_ms) = Self::export_range(source_meta.duration_ms, start_ms, end_ms, mode)?;

        // Validate before reserving a destination: a refused track choice
        // must not strand an empty MP4. Reconcile legacy sidecars against the
        // actual source so old trims cannot invent missing audio streams.
        let source_audio_count = export::audio_track_count(paths.mp4())?;
        let source_audio_names: Vec<String> = (0..source_audio_count)
            .map(|index| source_meta.audio_tracks.get(index).cloned()
                .unwrap_or_else(|| if index == 0 { "Mix".to_string() } else { format!("Piste {}", index + 1) }))
            .collect();
        let kept: Option<Vec<export::TrackChoice>> = match tracks {
            Some(tracks) => Some(self.validate_export_tracks(paths.mp4(), tracks)?
                .into_iter().filter(|track| track.gain_percent > 0).collect()),
            None if output == clippy_proto::command::ExportOutput::Share => Some(
                (source_audio_count > 0).then_some(export::TrackChoice {
                    ordinal: 0,
                    gain_percent: 100,
                }).into_iter().collect()
            ),
            None => None,
        };

        // Next to the clip it was cut from, named after it: "VALORANT
        // 2026-11-02 14-27-22 (découpé).mp4".
        let dir = paths.dir();
        let title = match (shaping.size_limit_mb, shaping.vertical_center.is_some()) {
            (Some(mb), true) => format!("{} (9:16, {mb} Mo)", source_meta.title),
            (Some(mb), false) => format!("{} ({mb} Mo)", source_meta.title),
            (None, true) => format!("{} (9:16)", source_meta.title),
            (None, false) => format!("{} (découpé)", source_meta.title),
        };
        let stem = clippy_core::store::sanitize_file_stem(&title).unwrap_or_else(|| library::now_id_stamp());
        let dest = clippy_core::store::reserve_clip_file(dir, &stem)?;
        let new_id = library::now_id_stamp();

        // `reserve_clip_file` claimed `dest` by creating it empty, so that a
        // clip taken with the hotkey mid-export cannot be handed the same name
        // and have its footage truncated out from under it. The reservation
        // is ours, so cleaning it up is ours too.
        //
        // This does *not* violate `export.rs`'s `BeforeCreating` rule, and the
        // distinction is the crux of why it is safe. That rule says: when an
        // attempt fails before `MFCreateSinkWriterFromURL`, whatever sits at
        // `dest` belongs to whoever put it there, so `export.rs` must not
        // delete it. Here the daemon *is* whoever put it there — it created
        // that file itself, one statement ago, for this export. So the two
        // halves stay honest: `export.rs` never touches a `dest` it did not
        // create, and `state.rs` cleans up the file it did.
        let remix_output = match output {
            clippy_proto::command::ExportOutput::Master => export::RemixOutput::MixAndStems,
            clippy_proto::command::ExportOutput::Share => export::RemixOutput::MixOnly,
        };
        let exported = if mode == "precise" {
            export::export_precise_with_control(export::PreciseExport {
                source: paths.mp4(), dest: dest.as_path(), start_ms, end_ms,
                tracks: kept.as_deref(), output: remix_output,
                size_limit_bytes: shaping.size_limit_mb.map(|mb| u64::from(mb) * 1_000_000),
                vertical_center: shaping.vertical_center,
            }, control)
        } else { match &kept {
            None => export::export_fast_with_control(export::FastExport {
                source: paths.mp4(),
                dest: dest.as_path(),
                start_ms,
                end_ms,
            }, control),
            Some(kept) => export::export_remix_with_control(export::RemixExport {
                source: paths.mp4(),
                dest: dest.as_path(),
                start_ms,
                end_ms,
                tracks: kept,
                output: remix_output,
            }, control),
        }};
        let outcome = match exported {
            Ok(outcome) => outcome,
            Err(e) => {
                // `NotFound` is the ordinary case, not a problem: an attempt
                // that got as far as creating `dest` deletes it on the way out
                // (the core export cleans up), so there is usually nothing left.
                match std::fs::remove_file(dest.as_path()) {
                    Ok(()) => {}
                    Err(remove) if remove.kind() == std::io::ErrorKind::NotFound => {}
                    // Worth a line rather than a silent `let _`: a reservation
                    // that will not delete is a zero-byte .mp4 in the clips
                    // folder, and while `library::scan` skips those so it can
                    // never become a broken grid card, it does hold the id
                    // against a clip saved in that same second.
                    Err(remove) => tracing::warn!(
                        clip = %new_id,
                        error = %remove,
                        "could not remove the reservation left by a failed export"
                    ),
                }
                return Err(e);
            }
        };

        if control.is_cancelled() {
            if let Err(error) = std::fs::remove_file(dest.as_path()) {
                if error.kind() != std::io::ErrorKind::NotFound { tracing::warn!(%error, "could not remove cancelled export"); }
            }
            return Err(export::ExportCancelled.into());
        }

        let mut meta = ClipMeta {
            id: new_id.clone(),
            title: dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            created: library::now_rfc3339_local(),
            // Measured from written samples, including any boundary snapping.
            duration_ms: outcome.duration_ms,
            bytes: std::fs::metadata(dest.as_path()).map(|m| m.len()).unwrap_or(0),
            width: outcome.frame_size.map_or(source_meta.width, |(w, _)| w),
            height: outcome.frame_size.map_or(source_meta.height, |(_, h)| h),
            fps: source_meta.fps,
            encoder: if mode == "precise" { "Media Foundation H.264".into() } else { source_meta.encoder.clone() },
            has_audio: outcome.has_audio,
            favorite: false,
            audio_tracks: match &kept {
                _ if !outcome.has_audio => Vec::new(),
                _ if output == clippy_proto::command::ExportOutput::Share => vec!["Mix".to_string()],
                None => source_audio_names.clone(),
                Some(kept) => exported_track_names(&source_audio_names, kept),
            },
            game: source_meta.game.clone(),
            folder: source_meta.folder.clone(),
        };
        // A record that will not write is not a failed export: the `.mp4` is
        // on disk and the next scan adopts it, losing only its track names.
        match self.store.insert(&meta, &dest) {
            Ok(id) => meta.id = id,
            Err(e) => tracing::warn!(clip = %new_id, error = %format!("{e:#}"), "export saved without a record"),
        }
        // The source's thumbnail, copied. It is the source's first frame rather
        // than the trim's, which is wrong in the strict sense — but a grid card
        // with no image at all reads as a broken clip, and this export has no
        // decoder to pull the real frame from. Best-effort: the UI falls back to
        // the video itself.
        if let Ok(jpeg) = std::fs::read(paths.thumb()) {
            self.store.write_thumb(&meta.id, &jpeg);
        }

        // The same call `Daemon::clip` makes, which is what puts the export
        // into the live library cache and broadcasts `clip_saved`. Broadcasting
        // by hand here would announce a clip that `library.list` — cache-only
        // by design, spec §5.2 — could not then show, and would skip the
        // library-ceiling check that the bytes this export just added are
        // exactly what it exists to notice.
        //
        // Without the chime, though. It is the sound that has meant "the
        // replay hotkey fired" since the feature shipped, and an export is a
        // thing the user just asked for in a window they are looking at --
        // hearing the capture sound while armed and in-game reads as a clip
        // they did not take.
        self.record_saved_clip(&meta, PlaySound::No);
        Ok(meta)
    }

    /// Checks the tracks an export asked to keep against the clip's own audio
    /// streams. An index the clip does not have is an error, not a skipped
    /// track: the user would get a clip without a sound they chose.
    fn validate_export_tracks(
        &self,
        source: &Path,
        tracks: &[clippy_proto::command::ExportTrack],
    ) -> Result<Vec<export::TrackChoice>> {
        const MAX_TRACKS: usize = 16;
        let available = export::audio_track_count(source)?;
        if tracks.len() > MAX_TRACKS {
            bail!("au plus {MAX_TRACKS} pistes audio par export");
        }
        if available > 1 && tracks.len() > 1 && tracks.iter().any(|track| track.index == 0) {
            bail!("le Mix contient déjà les pistes séparées ; choisissez le Mix seul ou les pistes à mélanger");
        }
        let mut kept: Vec<export::TrackChoice> = Vec::new();
        for track in tracks {
            if track.index >= available {
                bail!("ce clip n’a pas de piste audio {} (il en a {available})", track.index);
            }
            if kept.iter().any(|k| k.ordinal == track.index) {
                bail!("la piste audio {} est demandée deux fois", track.index);
            }
            kept.push(export::TrackChoice { ordinal: track.index, gain_percent: track.gain });
        }
        Ok(kept)
    }

    /// `library.tracks`: each audio track of a clip as a file of its own, for
    /// the editor to play side by side. Rebuildable derivatives live in the
    /// library cache, outside the clips folder.
    pub(crate) fn clip_tracks(&self, clip_id: &str) -> Result<Value> {
        let paths = self.paths_for(clip_id)?;
        if !paths.mp4().is_file() {
            bail!("clip {clip_id} introuvable");
        }
        let names = self
            .lock_library()
            .iter()
            .find(|clip| clip.id == clip_id)
            .map(|clip| clip.audio_tracks.clone())
            .unwrap_or_default();
        let count = export::audio_track_count(paths.mp4())?;
        let _one_at_a_time =
            self.track_files.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::fs::create_dir_all(paths.track_cache())
            .context("création du cache audio de l’éditeur")?;
        let mut out = Vec::new();
        // Mix is already in the MP4 and is not an editable stem.
        let first = usize::from(count > 1);
        for ordinal in first..count {
            let file = paths.track(ordinal);
            if !track_cache_is_fresh(paths.mp4(), &file) {
                let partial = paths.track_partial(ordinal);
                let extracted = export::extract_audio_track(paths.mp4(), ordinal, &partial)
                    .and_then(|()| {
                        std::fs::rename(&partial, &file)
                            .with_context(|| format!("impossible d’enregistrer la piste {ordinal}"))
                    });
                if let Err(error) = extracted {
                    if let Err(cleanup) = std::fs::remove_file(&partial) {
                        if cleanup.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!(path = %partial.display(), %cleanup, "could not remove partial editor audio");
                        }
                    }
                    return Err(error).with_context(|| {
                        format!("impossible d’extraire la piste {ordinal} de {clip_id}")
                    });
                }
            }
            let mut entry = Map::new();
            entry.insert("index".to_string(), Value::from(ordinal));
            entry.insert(
                "name".to_string(),
                Value::from(
                    names.get(ordinal).cloned().unwrap_or_else(|| format!("Piste {}", ordinal + 1)),
                ),
            );
            entry.insert(
                "file".to_string(),
                // Absolute: the cache is outside the clips folder.
                Value::from(file.to_string_lossy().into_owned()),
            );
            out.push(Value::Object(entry));
        }
        let mut reply = Map::new();
        reply.insert("clip_id".to_string(), Value::from(clip_id));
        reply.insert("tracks".to_string(), Value::Array(out));
        Ok(Value::Object(reply))
    }

    /// The choke point every id-taking command goes through: [`ClipPaths`] is
    /// sealed in its own submodule and [`ClipPaths::for_id`] — which validates
    /// the id with `library::is_valid_id` before anything else, then takes the
    /// file from the library index — is the only way to build one. No path it
    /// hands out can have come from the request itself.
    fn paths_for(&self, id: &str) -> Result<ClipPaths> {
        ClipPaths::for_id(&self.store, id)
    }

    /// What the library knows of a clip, or what its file says when the index
    /// has nothing (a record lost with a broken index).
    fn source_meta(&self, id: &str, mp4: &Path) -> Result<ClipMeta> {
        if let Some(entry) = self.store.entry(id) {
            return Ok(entry.meta);
        }
        let probe = export::probe(mp4).with_context(|| format!("{id} has no readable metadata"))?;
        Ok(ClipMeta {
            id: id.to_string(),
            title: mp4.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            duration_ms: probe.duration_ms,
            width: probe.width,
            height: probe.height,
            fps: probe.fps,
            has_audio: probe.audio_tracks > 0,
            created: library::created_from_id(id),
            bytes: std::fs::metadata(mp4).map(|m| m.len()).unwrap_or(0),
            encoder: String::new(),
            favorite: false,
            audio_tracks: (1..=probe.audio_tracks).map(|n| format!("Piste {n}")).collect(),
            game: String::new(),
            folder: String::new(),
        })
    }

    /// Applies `edit` to the listed row of `id`, after the disk has changed.
    fn edit_listed(&self, id: &str, edit: impl FnOnce(&mut ClipMeta)) -> Result<ClipMeta> {
        let mut library = self.lock_library();
        let Some(found) = library.iter_mut().find(|clip| clip.id == id) else {
            bail!("clip {id} introuvable dans la bibliothèque");
        };
        edit(found);
        Ok(found.clone())
    }

    // Poisoning is recovered from rather than unwrapped: `lock().unwrap()` is a
    // panicking call on a path reachable straight from a socket message, and
    // the Global Constraints forbid those outright. See the same note in
    // `clients.rs`.
    fn lock_armed(&self) -> MutexGuard<'_, Option<Armed>> {
        self.armed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_game(&self) -> MutexGuard<'_, GameWatch> {
        self.game.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn lock_config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_library(&self) -> MutexGuard<'_, Vec<ClipMeta>> {
        self.library.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The converted sound cache to play, if there is one, given an already-held
/// `Config` and the daemon's `config_path`.
///
/// Pure and lock-free by design: [`Daemon::custom_sound_source`] takes the
/// `config` lock itself and delegates here, while [`Daemon::record_saved_clip`]
/// reads `clip_sound` and resolves the path under one guard it already holds.
/// Neither caller may lock `config` a second time from in here without
/// deadlocking a non-reentrant `Mutex`, so this function must never do so.
///
/// `None` covers two different situations a caller does not need to tell
/// apart: `clip_sound_path` is empty, meaning the user is on the built-in
/// chime, and `clip_sound_path` names a file but there is nowhere a converted
/// copy could have been cached (`config_path` is `None` — `%APPDATA%` unset).
/// Either way [`crate::sound::play`] is told the same thing, `None`, and
/// plays the built-in chime.
fn resolve_sound(config: &Config, config_path: Option<&Path>) -> Option<PathBuf> {
    if config.clip_sound_path.trim().is_empty() {
        None
    } else {
        config_path.map(Config::sound_cache_path)
    }
}

/// The decision [`Daemon::repair_sound_cache`] acts on: does the cache on
/// disk need rebuilding before it can be trusted to match `chosen`?
///
/// Pulled out as a pure function, with no filesystem access of its own, so it
/// can be tested without Media Foundation -- everything about *deciding*
/// whether to rebuild is ordinary string and boolean logic; only the rebuild
/// itself needs MF. `sidecar` is `None` for "missing, unreadable, or never
/// written" -- callers are not expected to distinguish those, because none of
/// them earn a different answer here.
///
/// A cache file existing on its own says nothing about which sound produced
/// it. Three ordinary situations reach this guard with a cache present that
/// does not match `chosen`, and none of them self-heal without it: a
/// `config.toml` edited by hand to name a different file than the one last
/// converted, a process that died between saving the config and writing the
/// cache, and a cache write that failed after the sidecar from the *previous*
/// sound had already been cleaned up (or vice versa). The sidecar is what
/// turns "a cache exists" into "a cache exists *for this sound*": a rebuild
/// is needed exactly when there is a configured sound and the cache cannot be
/// shown to match it, i.e. is missing, or its sidecar is missing, or its
/// sidecar names something other than `chosen`.
fn needs_sound_cache_rebuild(chosen: &str, cache_exists: bool, sidecar: Option<&str>) -> bool {
    if chosen.is_empty() {
        // Nothing configured, so there is nothing to rebuild -- `resolve_sound`
        // never looks at the cache when `clip_sound_path` is empty, and
        // `config.set`'s reset path is what deletes it.
        return false;
    }
    if !cache_exists {
        return true;
    }
    sidecar != Some(chosen)
}

/// Writes the converted sound beside the config file, through a temporary
/// name, then records `source` in the sidecar beside it.
///
/// Temp-then-rename rather than a direct write: a half-written cache is a valid
/// path with an invalid file at it, and `PlaySound` answers that with silence.
/// A rename on the same volume is atomic, so the cache is either the old sound
/// or the new one and never a prefix of either.
///
/// An `Err` from this function means exactly one thing: the cache file is not
/// in place, because the temp write or the rename that publishes it failed.
/// That is the only case a caller needs to reason about -- `set_config`'s
/// caller deletes whatever is at the final path on `Err` precisely because an
/// `Err` here guarantees that file, if it exists at all, is stale (built for
/// some earlier sound, or never built).
///
/// The sidecar is written only after that rename lands, so it is never in
/// place ahead of the cache it describes -- and its write is best-effort. A
/// cache that landed correctly must never be thrown away over a bookkeeping
/// file failing to write beside it, so a failed sidecar write is logged and
/// swallowed rather than returned: a missing sidecar is already one of
/// [`needs_sound_cache_rebuild`]'s own rebuild conditions, so the next
/// startup's `repair_sound_cache` heals it without anyone having to notice in
/// between.
fn write_sound_cache(config_path: &std::path::Path, wav: &[u8], source: &str) -> Result<()> {
    let final_path = Config::sound_cache_path(config_path);
    let temp_path = final_path.with_extension("wav.tmp");
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    std::fs::write(&temp_path, wav)
        .with_context(|| format!("could not write {}", temp_path.display()))?;
    if let Err(e) = std::fs::rename(&temp_path, &final_path) {
        // Best-effort: a failed rename must not also leave the temp file
        // behind. Nothing else ever revisits `clip-sound.wav.tmp`, so
        // without this an error return here would litter it permanently.
        let _ = std::fs::remove_file(&temp_path);
        return Err(e).with_context(|| format!("could not replace {}", final_path.display()));
    }
    let sidecar_path = Config::sound_src_path(config_path);
    if let Err(e) = std::fs::write(&sidecar_path, source) {
        // Not `?`: the cache above is already on disk and correct, and
        // failing this call over the sidecar would make its caller delete
        // that good cache believing it was never written at all. A missing
        // sidecar reads as "needs rebuild" on its own, so this heals itself
        // at the next startup instead.
        tracing::warn!(
            error = %format!("{e:#}"),
            path = %sidecar_path.display(),
            "the sound cache landed but its sidecar could not be written; the next startup \
             will rebuild it"
        );
    }
    Ok(())
}


/// What the auto-clip service reads from the config on every moment.
#[derive(Debug, Clone, Copy)]
pub struct AutoClipSettings {
    pub enabled: bool,
    pub min_kills: u32,
    pub replay_seconds: u32,
}

/// How an export reshapes the picture and its size, beyond the cut itself.
/// The banner for a clip the hotkey (or auto-clip) just saved: what was saved
/// and how long it is.
#[cfg(test)]
mod place_tests {
    use super::*;

    fn valorant() -> Sighting {
        Sighting {
            exe: "valorant-win64-shipping.exe".into(),
            name: "VALORANT".into(),
            left: 0,
            top: 0,
            known: true,
        }
    }

    #[test]
    fn a_desktop_session_files_the_clip_under_the_game_in_front() {
        let seen = valorant();
        let place = place_for(true, false, Some(&seen), None).expect("a place");
        assert_eq!(place.folder, "VALORANT");
        assert_eq!(place.game, "VALORANT");
    }

    #[test]
    fn no_game_grouping_or_a_game_session_keeps_the_session_folder() {
        let seen = valorant();
        assert_eq!(place_for(false, false, Some(&seen), None), None);
        assert_eq!(place_for(true, true, Some(&seen), None), None);
        assert_eq!(place_for(true, false, None, None), None);
    }
}

pub(crate) fn clip_banner_message(meta: &ClipMeta) -> crate::banner::Message {
    // A clip nobody named yet is titled after its id (`clip_<stamp>`), which
    // says nothing; the game it was recorded in says more.
    let name = if meta.title.is_empty() || meta.title == format!("clip_{}", meta.id) {
        if meta.game.is_empty() { "Bureau" } else { meta.game.as_str() }
    } else {
        meta.title.as_str()
    };
    crate::banner::Message {
        title: "Clip enregistré".into(),
        detail: format!("{name} · {}", crate::banner::duration_label(meta.duration_ms)),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Shaping {
    pub size_limit_mb: Option<u32>,
    pub vertical_center: Option<u16>,
}

#[cfg(test)]
mod banner_tests {
    use super::*;

    fn meta(id: &str, title: &str, game: &str) -> ClipMeta {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "created": "2026-11-01T12:00:00+01:00",
            "duration_ms": 15_000, "bytes": 1, "width": 1920, "height": 1080, "fps": 60,
            "encoder": "x", "has_audio": true, "favorite": false, "game": game,
        }))
        .unwrap()
    }

    #[test]
    fn the_banner_names_the_game_rather_than_a_default_title() {
        let id = "20261101_120000";
        assert_eq!(clip_banner_message(&meta(id, &format!("clip_{id}"), "")).detail, "Bureau · 0:15");
        assert_eq!(clip_banner_message(&meta(id, &format!("clip_{id}"), "Valorant")).detail, "Valorant · 0:15");
        assert_eq!(clip_banner_message(&meta(id, "Double kill · Valorant", "Valorant")).detail, "Double kill · Valorant · 0:15");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn precise_export_accepts_single_frame_but_fast_keeps_its_floor() {
        assert_eq!(super::Daemon::export_range(1000, 17, 33, "precise").unwrap(), (17, 33));
        assert!(super::Daemon::export_range(1000, 17, 33, "fast").is_err());
        assert_eq!(super::Daemon::export_range(1000, 950, 1100, "precise").unwrap(), (950, 1000));
        assert!(super::Daemon::export_range(1000, 1000, 1100, "precise").is_err());
        assert!(super::Daemon::export_range(0, 0, 1, "precise").is_err());
    }

    #[test]
    fn editor_derivatives_stay_in_the_library_cache() {
        let root = std::env::temp_dir().join(format!("clippy-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = super::Store::new(root.join("library.json"), root.join("cache"));
        let mp4 = root.join("clips").join("VALORANT 2026-01-01 10-00-00.mp4");
        let meta = clippy_proto::ClipMeta {
            id: "20260101_100000".into(), title: String::new(), created: String::new(), duration_ms: 0,
            bytes: 0, width: 0, height: 0, fps: 0, encoder: String::new(), has_audio: true,
            favorite: false, audio_tracks: Vec::new(), game: String::new(), folder: String::new(),
        };
        store.insert(&meta, &mp4).unwrap();
        let paths = super::ClipPaths::for_id(&store, "20260101_100000").unwrap();
        let tracks = root.join("cache").join("tracks");
        assert_eq!(paths.mp4(), mp4);
        assert_eq!(paths.track(2), tracks.join("20260101_100000.t2.m4a"));
        assert_eq!(paths.track_partial(2), tracks.join("20260101_100000.t2.part.m4a"));
        assert_eq!(paths.thumb(), root.join("cache").join("thumbs").join("20260101_100000.jpg"));
        assert!(super::ClipPaths::for_id(&store, "20260101_100001").is_err(), "not in the index");
        assert!(super::ClipPaths::for_id(&store, "../x").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn editor_cache_requires_nonempty_audio_newer_than_the_master() {
        use std::time::{Duration, SystemTime};
        let dir =
            std::env::temp_dir().join(format!("clippy-cache-freshness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("clip.mp4");
        let cached = dir.join("clip.t1.m4a");
        std::fs::write(&source, b"master").unwrap();
        assert!(!super::track_cache_is_fresh(&source, &cached));
        std::fs::write(&cached, b"audio").unwrap();
        let now = SystemTime::now();
        std::fs::OpenOptions::new().write(true).open(&source).unwrap().set_modified(now).unwrap();
        std::fs::OpenOptions::new().write(true).open(&cached).unwrap().set_modified(now - Duration::from_secs(10)).unwrap();
        assert!(!super::track_cache_is_fresh(&source, &cached));
        std::fs::OpenOptions::new().write(true).open(&cached).unwrap().set_modified(now + Duration::from_secs(10)).unwrap();
        assert!(super::track_cache_is_fresh(&source, &cached));
        std::fs::write(&cached, b"").unwrap();
        assert!(!super::track_cache_is_fresh(&source, &cached));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use std::path::{Path, PathBuf};

    use super::*;

    fn meta(id: &str) -> ClipMeta {
        ClipMeta {
            id: id.to_string(),
            title: format!("clip_{id}"),
            created: "2026-07-26T10:00:00+03:00".into(),
            duration_ms: 15_000,
            bytes: 5,
            width: 1920,
            height: 1080,
            fps: 60,
            encoder: "test".into(),
            has_audio: true,
            favorite: false,
            audio_tracks: Vec::new(),
            game: String::new(),
            folder: String::new(),
        }
    }

    fn path_of(daemon: &Daemon, id: &str) -> PathBuf {
        daemon.store.entry(id).expect("a recorded clip").path
    }

    fn fixture(name: &str) -> (Daemon, PathBuf) {
        let dir = std::env::temp_dir().join(format!("clippy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for id in ["20260726_100000", "20260726_110000", "20260726_120000"] {
            std::fs::write(clippy_core::library::mp4_path(&dir, id), b"video").unwrap();
            clippy_core::library::write_sidecar(&dir, &meta(id)).unwrap();
        }
        let config = Config { clip_dir: dir.to_string_lossy().into_owned(), ..Config::default() };
        let daemon = Daemon::new_at(config, None);
        daemon.rescan_library().unwrap();
        (daemon, dir)
    }

    /// A daemon that touches neither of the developer's real files, for the
    /// tests that need no clips at all.
    ///
    /// These were written as `Daemon::new(Config::default())`, which is the
    /// production constructor: it resolves `config_path` from `Config::path()`
    /// — the live `%APPDATA%\clippy\config.toml` — and runs the startup library
    /// scan over the real `Videos\Clippy`. So `disarm_when_idle_is_not_an_error`
    /// and its neighbours walked the user's actual footage on every
    /// `cargo test --workspace`, and the `config.set` tests were aimed at their
    /// actual settings. Nothing was ever written, because those tests reject
    /// before the write — but that is a property of the assertions, not of the
    /// fixture, and this crate already had the seam (`Daemon::new_at`) in three
    /// other places with a doc comment explaining exactly why it matters.
    ///
    /// `config_path` is `None`, so a `config.set` has nowhere to go. `clip_dir`
    /// is a scratch path that is deliberately never created — `library::scan`
    /// reports a missing directory as an empty library — so this does no
    /// filesystem I/O and leaves nothing to clean up.
    fn idle(name: &str, config: Config) -> Daemon {
        let dir = std::env::temp_dir().join(format!("clippy-idle-{name}-{}", std::process::id()));
        Daemon::new_at(Config { clip_dir: dir.to_string_lossy().into_owned(), ..config }, None)
    }

    /// A daemon whose `config.set` really writes, to a scratch file the test
    /// owns.
    ///
    /// Both halves of the isolation are required, and a scratch `%APPDATA%`
    /// would supply neither: `Daemon::new_at` takes the config path directly,
    /// and `clip_dir` is a key *inside* that config whose empty default
    /// resolves to the developer's real `%USERPROFILE%\Videos\Clippy` no matter
    /// what `%APPDATA%` says. A test that writes a config must own both or it
    /// is editing the settings of whoever ran `cargo test`.
    fn writable(name: &str) -> (Daemon, PathBuf) {
        let dir = std::env::temp_dir().join(format!("clippy-vol-{name}-{}", std::process::id()));
        // The name is only unique per process id, and a test that fails an
        // assertion never reaches its `cleanup`. Without this, a reused pid
        // inherits the previous run's `config.toml` and clips.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let config = Config {
            clip_dir: dir.join("clips").to_string_lossy().into_owned(),
            ..Config::default()
        };
        (Daemon::new_at(config, Some(dir.join("config.toml"))), dir)
    }

    /// Removes a `writable` scratch directory. Best-effort: a leaked temp
    /// directory is not worth failing a passing test over.
    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A daemon whose `config.set` writes to a scratch file rather than the
    /// developer's real `%APPDATA%\clippy\config.toml`, returning both halves of
    /// the isolation the sound tests below need: the scratch config path
    /// (`Config::sound_cache_path` derives the cache from it) and the scratch
    /// `clip_dir`.
    ///
    /// `dispatch.rs` has a helper of the same name and shape, but it is
    /// private to that module's own test module and out of scope here, so
    /// this is state.rs's own copy rather than a shared import.
    fn with_scratch_config(name: &str) -> (Daemon, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("clippy-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let config = Config { clip_dir: dir.to_string_lossy().into_owned(), ..Config::default() };
        (Daemon::new_at(config, Some(path.clone())), path, dir)
    }

    /// `config.get` must report the registry, not the file (spec §7.3).
    ///
    /// Loads the daemon with the *opposite* of whatever the registry actually
    /// says, so the two disagree and only one answer can be right. Asserting
    /// against `is_enabled()` alone would re-implement the override in its own
    /// assertion and pass no matter what `config_json` did; this fails the
    /// moment the override line is dropped, on a machine in either state.
    ///
    /// Read-only — it never writes a Run entry.
    #[test]
    fn config_get_reports_the_registry_and_not_the_config_file() {
        let truth = crate::autostart::is_enabled();
        let daemon = idle("autostart", Config { autostart: !truth, ..Config::default() });

        let json = daemon.config_json().unwrap();
        assert_eq!(
            json.get("autostart"),
            Some(&Value::Bool(truth)),
            "the Run entry decides, whatever the config file holds"
        );
    }

    #[test]
    fn list_pages_newest_first_and_clamps_past_the_end() {
        let (daemon, dir) = fixture("list");

        let page = daemon.list(0, 2);
        assert_eq!(page.total, 3, "total is the unpaged count — the UI needs it for \"3 of 47\"");
        let ids: Vec<&str> = page.clips.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["20260726_120000", "20260726_110000"]);

        let page = daemon.list(2, 50);
        assert_eq!(page.clips.len(), 1, "a short final page, not an error");

        let page = daemon.list(99, 50);
        assert!(page.clips.is_empty(), "an offset past the end is empty, not a panic");
        assert_eq!(page.total, 3);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn large_library_pages_share_the_full_size_and_detect_changes() {
        let daemon = idle("large-library", Config::default());
        *daemon.lock_library() = (0..1205)
            .map(|index| ClipMeta { bytes: 5_000_000, ..meta(&format!("clip-{index}")) })
            .collect();

        let first = daemon.list(0, 200);
        let last = daemon.list(1200, 200);
        assert_eq!(first.total, 1205);
        assert_eq!(first.clips.len(), 200);
        assert_eq!(last.clips.len(), 5);
        assert_eq!(first.total_bytes, 6_025_000_000);
        assert_eq!(last.total_bytes, first.total_bytes);
        assert_eq!(last.snapshot, first.snapshot);

        daemon.lock_library()[0].favorite = true;
        let edited = daemon.list(0, 200);
        assert_ne!(edited.snapshot, first.snapshot);
        assert_eq!(edited.total_bytes, first.total_bytes);
        daemon.lock_library().pop();
        let shortened = daemon.list(0, 200);
        assert_ne!(shortened.snapshot, edited.snapshot);
        assert_eq!(shortened.total, 1204);
        assert_eq!(shortened.total_bytes, 6_020_000_000);
    }

    /// The security boundary. Every id-taking command runs through the same
    /// check; if one forgets, this catches it.
    ///
    /// `delete` and `reveal` are proven by the `canary` id, which really would
    /// hit `canary.mp4` if validation were skipped — that is the load-bearing
    /// case. `rename` and `favorite` cannot be proven the same way: both go
    /// through `edit_meta`, whose cache lookup fails on all five of these ids
    /// regardless of whether `is_valid_id` ran (none of them name a clip
    /// `library::scan` would ever have adopted, since `scan` itself filters
    /// through `is_valid_id`). So for those two, `is_err()` alone would pass
    /// even with the check ripped out — what has to be asserted is the error
    /// *text*, which distinguishes "refused at the boundary" from "fell
    /// through to a `NotFound` lookup".
    #[test]
    fn every_id_command_rejects_a_traversal_attempt() {
        let (daemon, dir) = fixture("traversal");
        let canary = dir.join("canary.mp4");
        std::fs::write(&canary, b"must survive").unwrap();

        for evil in [
            "../../../Windows/System32/config/SAM",
            r"..\..\secrets",
            "x/../y",
            "canary",
            r"C:\Windows\System32\drivers\etc\hosts",
        ] {
            assert!(daemon.delete(evil).is_err(), "delete accepted {evil:?}");

            let rename_err = daemon.rename(evil, "x").unwrap_err().to_string();
            assert!(
                rename_err.contains("n’est pas un identifiant de clip valide"),
                "rename on {evil:?} must be refused at validation, not fall through to a lookup: {rename_err}"
            );

            let favorite_err = daemon.set_favorite(evil, true).unwrap_err().to_string();
            assert!(
                favorite_err.contains("n’est pas un identifiant de clip valide"),
                "favorite on {evil:?} must be refused at validation, not fall through to a lookup: {favorite_err}"
            );

            assert!(daemon.reveal(evil).is_err(), "reveal accepted {evil:?}");
        }
        assert!(canary.exists(), "a rejected id must not have touched the filesystem");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_renames_the_file_and_keeps_the_id() {
        let (daemon, dir) = fixture("rename");
        let id = "20260726_110000";
        let before = path_of(&daemon, id);

        daemon.rename(id, "Ace on Ascent").unwrap();

        assert!(dir.join("Ace on Ascent.mp4").is_file(), "the title is the file name");
        assert!(!before.exists());
        assert_eq!(path_of(&daemon, id), dir.join("Ace on Ascent.mp4"), "same id, new file");
        assert!(daemon.rename("20260726_120000", "ace on ascent").is_err(), "never over another clip");

        let cached = daemon.list(0, 50);
        let entry = cached.clips.iter().find(|c| c.id == id).unwrap();
        assert_eq!(entry.title, "Ace on Ascent", "the cache must not go stale");

        assert!(daemon.rename(id, "   ").is_err(), "a blank title is not a rename");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_removes_the_file_its_record_and_its_thumbnail() {
        let (daemon, dir) = fixture("delete");
        let id = "20260726_100000";
        daemon.store.write_thumb(id, b"jpeg");
        let mp4 = path_of(&daemon, id);

        daemon.delete(id).unwrap();

        assert!(!mp4.exists());
        assert!(!daemon.store.thumb(id).exists());
        assert!(daemon.store.entry(id).is_none(), "the record goes with it");
        assert_eq!(daemon.list(0, 50).total, 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An id that is valid syntax but names no clip is a clean error, not a
    /// panic and not a silent success.
    #[test]
    fn an_unknown_but_wellformed_id_is_a_clean_error() {
        let (daemon, dir) = fixture("unknown");

        let err = daemon.delete("20991231_235959").unwrap_err().to_string();
        assert!(err.contains("20991231_235959"), "the error should name the clip: {err}");
        assert!(daemon.rename("20991231_235959", "x").is_err());
        assert_eq!(daemon.list(0, 50).total, 3, "a failed delete changed nothing");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A clip deleted in Explorer while the daemon keeps running: `library.list`
    /// still shows it, because cache-only reads are by design (spec §5.2). But
    /// there is no `library.refresh` in this build, so a `delete` that fails
    /// with `NotFound` has to evict the cache entry itself — otherwise the row
    /// is stuck forever and `total` stays wrong until the daemon restarts, and
    /// a `rename` on the ghost would write an orphan `.json` for an `.mp4` that
    /// no longer exists.
    #[test]
    fn a_clip_missing_behind_the_daemons_back_is_evicted_by_a_failed_delete() {
        let (daemon, dir) = fixture("ghost");
        let id = "20260726_100000";
        daemon.store.write_thumb(id, b"jpeg");
        std::fs::remove_file(path_of(&daemon, id)).unwrap();

        assert_eq!(daemon.list(0, 50).total, 3, "the cache does not know yet — by design");

        let err = daemon.delete(id).unwrap_err().to_string();
        assert!(err.contains(id), "the error should name the clip: {err}");
        assert_eq!(
            daemon.list(0, 50).total,
            2,
            "a failed delete must still evict the ghost row from the cache"
        );

        // The companions must go with it. Once the row is evicted no client can
        // name this id again — `list` reads the cache and `scan` skips a
        // sidecar with no `.mp4` — so anything left here is litter the daemon
        // could never be asked to clean up.
        assert!(daemon.store.entry(id).is_none(), "the orphaned record must not be left behind");
        assert!(!daemon.store.thumb(id).exists(), "the orphaned thumbnail must not be left behind");

        // A second delete of the same id is still a clean error, not a panic
        // and not a different message because the entry is now gone from the
        // cache too.
        let err_again = daemon.delete(id).unwrap_err().to_string();
        assert!(err_again.contains(id), "a repeat delete is still a clean error: {err_again}");
        assert_eq!(daemon.list(0, 50).total, 2, "nothing further changes");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Favoriting is the same write path as renaming, so what it needs its own
    /// coverage for is that it edits *only* the flag — a shared helper that
    /// clobbered the title would otherwise pass every test above.
    #[test]
    fn favoriting_sets_the_flag_on_disk_without_disturbing_the_title() {
        let (daemon, dir) = fixture("favorite");
        let id = "20260726_120000";

        daemon.rename(id, "Clutch").unwrap();
        daemon.set_favorite(id, true).unwrap();

        assert!(daemon.store.entry(id).unwrap().meta.favorite, "kept in the index");
        assert_eq!(path_of(&daemon, id), dir.join("Clutch.mp4"), "favoriting must not undo a rename");
        let listed = daemon.list(0, 50);
        assert_eq!(listed.clips.iter().find(|c| c.id == id).unwrap().title, "Clutch");

        daemon.set_favorite(id, false).unwrap();
        let page = daemon.list(0, 50);
        let entry = page.clips.iter().find(|c| c.id == id).unwrap();
        assert!(!entry.favorite, "un-favoriting is the same path in reverse");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Disarming a daemon that was never armed is a no-op, not an error — a
    /// UI that reconnects and syncs its toggle must not see a spurious failure.
    #[test]
    fn disarm_when_idle_is_not_an_error() {
        let daemon = idle("disarm", Config::default());
        assert!(daemon.disarm().is_ok());
        assert!(!daemon.status().armed);
    }

    /// `disarm` broadcasts from inside `Daemon` rather than from `dispatch`,
    /// so the tray menu's disarm reaches an open UI. This pins the half of
    /// that which needs no capture hardware: the already-idle path must stay
    /// silent. A `disarmed` event for a daemon that was already idle would
    /// tell every client to re-read a state that never changed, and moving
    /// the broadcast above the `armed.take()` check is exactly how that
    /// happens.
    #[test]
    fn disarm_when_idle_broadcasts_nothing() {
        let daemon = idle("disarm-silent", Config::default());
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        daemon.clients.register(tx);

        assert!(!daemon.disarm().unwrap(), "a daemon that was never armed reports no transition");

        assert!(
            rx.try_recv().is_err(),
            "an already-idle disarm must not broadcast; a client saw an event"
        );
    }

    /// The status shape the UI binds to. Arming needs real hardware, so this
    /// covers only the disarmed branch — the armed branch is covered by the
    /// end-to-end gate in Task 8.
    #[test]
    fn idle_status_reports_config_not_engine_state() {
        let config = Config { replay_seconds: 30, monitor_index: 1, ..Config::default() };
        let daemon = idle("status", config);
        let status = daemon.status();
        assert!(!status.armed);
        assert_eq!(status.ring_seconds_total, 30);
        assert_eq!(status.monitor_index, 1);
        assert_eq!(status.ring_seconds_used, 0.0);
        assert!(status.encoder.is_none(), "a disarmed daemon has not chosen an encoder yet");
    }

    /// The exact key set from spec §4.2 — a UI is being built against it. This
    /// is the guard that a refactor of `DaemonStatus` cannot silently rename or
    /// drop one of them, and that `encoder` stays JSON `null` rather than
    /// becoming `""` when nothing is armed.
    #[test]
    fn the_status_wire_shape_is_the_published_one() {
        // Not routed through `idle`: this test asserts on `clip_dir` reaching the
        // wire verbatim, so it has to choose the literal itself. `new_at(.., None)`
        // still keeps it off the real config file, and `D:\Clips` is nobody's real
        // clip directory -- a missing one scans as an empty library.
        let daemon =
            Daemon::new_at(Config { clip_dir: r"D:\Clips".into(), ..Config::default() }, None);
        let json = daemon.status().to_json();
        let object = json.as_object().expect("status must serialize as a JSON object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "armed",
                "cache_dir",
                "clip_dir",
                "clip_hotkey_bound",
                "encoder",
                "game",
                "game_name",
                "monitor_index",
                "ring_seconds_total",
                "ring_seconds_used",
                "screenshot_hotkey_bound",
                "version",
                "waiting_for_game",
            ],
            "the status key set is published; a UI binds to exactly these"
        );

        assert_eq!(object.get("armed"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(object.get("encoder"), Some(&serde_json::Value::Null));
        // The two hotkey fields are asserted for presence above and for their
        // *values* in `the_hotkey_tri_state_survives_the_wire` below, not
        // here. `status_of` reads them from process-global statics that
        // `window.rs`'s pump tests write from other threads of this same test
        // binary, so a value assertion on a live `Daemon` would pass or fail
        // on test scheduling order.
        assert_eq!(object.get("clip_dir").and_then(|v| v.as_str()), Some(r"D:\Clips"));
        assert_eq!(object.get("version").and_then(|v| v.as_str()), Some(env!("CARGO_PKG_VERSION")));
    }

    fn sighting(exe: &str) -> Sighting {
        Sighting { exe: exe.to_string(), name: exe.to_string(), left: 0, top: 0, known: true }
    }

    /// In `game` mode, turning the replay on with no game in front must not
    /// record the desktop: it waits, and says it is waiting. This path starts no
    /// engine, so it runs without a GPU.
    #[test]
    fn game_mode_waits_for_a_game_instead_of_recording_the_desktop() {
        let daemon =
            idle("game-waits", Config { capture_mode: "game".into(), ..Config::default() });
        let outcome = daemon.arm().expect("waiting is not a failure");
        assert!(!outcome.status.armed, "nothing is recording");
        assert!(outcome.status.waiting_for_game);
        let status = daemon.status();
        assert!(status.waiting_for_game && !status.armed);
        assert_eq!(status.to_json()["waiting_for_game"], serde_json::Value::Bool(true));
    }

    /// Turning it off while it waits is a real change, so a UI that asked gets
    /// `true` and the wait is over; a second call finds nothing to stop.
    #[test]
    fn disarming_a_waiting_replay_ends_the_wait() {
        let daemon =
            idle("game-disarm", Config { capture_mode: "game".into(), ..Config::default() });
        daemon.arm().expect("waits");
        assert!(daemon.disarm().expect("disarm"));
        assert!(!daemon.status().waiting_for_game);
        assert!(!daemon.disarm().expect("disarm again"), "already off");
    }

    /// A loading screen swaps windows for a moment; that must not count as the
    /// game ending. Only a game gone for longer than the grace period does.
    #[test]
    fn a_game_out_of_sight_for_a_moment_is_still_the_game() {
        let daemon =
            idle("game-grace", Config { capture_mode: "game".into(), ..Config::default() });
        let start = Instant::now();
        daemon.game_tick(Some(sighting("eldenring.exe")), start);
        assert_eq!(daemon.status().game.as_deref(), Some("eldenring.exe"));
        daemon.game_tick(None, start + Duration::from_secs(2));
        assert_eq!(daemon.status().game.as_deref(), Some("eldenring.exe"), "inside the grace");
        daemon.game_tick(None, start + GAME_GRACE + Duration::from_secs(1));
        assert_eq!(daemon.status().game, None, "past the grace");
    }

    /// Auto-clip reads the game in front from the status in every mode; the
    /// desktop mode used to leave it empty, so Valorant was never watched.
    #[test]
    fn the_desktop_mode_still_knows_the_game_in_front_without_arming_for_it() {
        let daemon = idle("game-desktop", Config::default());
        let start = Instant::now();
        daemon.note_sighting(Some(sighting("valorant-win64-shipping.exe")), start);
        let status = daemon.status();
        assert_eq!(status.game.as_deref(), Some("valorant-win64-shipping.exe"));
        assert!(!status.armed && !status.waiting_for_game, "seeing a game arms nothing");
        daemon.note_sighting(None, start + GAME_GRACE + Duration::from_secs(1));
        assert_eq!(daemon.status().game, None);
    }

    #[test]
    fn an_unknown_capture_mode_is_refused() {
        let daemon = idle("game-mode-typo", Config::default());
        let mut values = Map::new();
        values.insert("capture_mode".into(), Value::from("gmae"));
        let error = daemon.set_config(&values).expect_err("typo");
        assert!(format!("{error:#}").contains("capture_mode"), "{error:#}");
    }

    /// The three answers "is this hotkey listening?" can have, as they reach a
    /// client. Built from a `DaemonStatus` literal rather than from a live
    /// `Daemon`, deliberately: `status_of` reads process-global statics that
    /// `window.rs`'s pump tests write from other threads of this binary, so
    /// only a hand-built struct can pin these values without racing them.
    ///
    /// `null` is the one that matters. It is what every client sees between
    /// the socket answering and the pump binding, and what any process
    /// without a window reports forever -- and a UI that read it as `false`
    /// would tell users their hotkey was taken at every single launch.
    #[test]
    fn the_hotkey_tri_state_survives_the_wire() {
        let status = |clip: Option<bool>, shot: Option<bool>| DaemonStatus {
            armed: false,
            encoder: None,
            monitor_index: 0,
            ring_seconds_used: 0.0,
            ring_seconds_total: 15,
            clip_dir: String::new(),
            cache_dir: String::new(),
            clip_hotkey_bound: clip,
            screenshot_hotkey_bound: shot,
            waiting_for_game: false,
            game: None,
            game_name: None,
        };
        let at = |clip, shot, key: &str| status(clip, shot).to_json()[key].clone();
        let null = serde_json::Value::Null;

        assert_eq!(at(None, None, "clip_hotkey_bound"), null, "untried is null, never false");
        assert_eq!(at(None, None, "screenshot_hotkey_bound"), null);
        assert_eq!(at(Some(true), None, "clip_hotkey_bound"), serde_json::Value::Bool(true));
        assert_eq!(at(Some(false), None, "clip_hotkey_bound"), serde_json::Value::Bool(false));
        assert_eq!(
            at(Some(false), None, "screenshot_hotkey_bound"),
            null,
            "the two keys are answered independently"
        );
    }

    /// Nothing is armed, so there are no counters to report and the stats
    /// thread has nothing to broadcast. `None` rather than a payload of zeroes:
    /// a live meter reading "0 frames, 0 dropped" about a daemon that is not
    /// capturing is a lie a UI would render as a stalled encoder.
    #[test]
    fn an_idle_daemon_has_no_stats_payload() {
        assert!(idle("stats-idle", Config::default()).stats_json().is_none());
    }

    /// `stats_seconds = 0` disables the periodic log line, not a subscriber's
    /// events — a client that asked to be told is told once a second.
    #[test]
    fn a_zero_stats_interval_still_ticks_for_a_subscriber() {
        let daemon = idle("stats-zero", Config { stats_seconds: 0, ..Config::default() });
        assert_eq!(daemon.stats_interval(), Duration::from_secs(1));

        let daemon = idle("stats-five", Config { stats_seconds: 5, ..Config::default() });
        assert_eq!(daemon.stats_interval(), Duration::from_secs(5), "an explicit interval wins");
    }

    /// The guard that makes [`NUMERIC_BOUNDS`] a *rule* rather than a list
    /// somebody once wrote. Every numeric key `Config` serializes has to have a
    /// bound, so a config key added later cannot reach `config.set` unchecked —
    /// the same way `config.get`'s key set is derived from `Config` rather than
    /// hand-written. It fails on the day the key is added, not on the day a
    /// user sets it to four billion.
    ///
    /// Its reach is exactly "numeric in a serialized `Config::default()`",
    /// which is every field `Config` has today (six `u32`s) but is not the
    /// same claim as "every future field". An `Option<u32>` defaulting to
    /// `None` serializes as `null`, so this loop would skip it and the key
    /// would reach `config.set` unbounded; a signed or floating key would be
    /// forced into the table here, but `check_ranges` reads values through
    /// `as_u64`, which is `None` for a negative, so the bound would never
    /// fire. Both want a change to `check_ranges`, not just a row in the
    /// table — noted here so a later reader does not take this test for more
    /// cover than it gives.
    #[test]
    fn every_numeric_config_key_is_range_checked() {
        let defaults = config_object(&Config::default()).expect("the config serializes");
        for (key, value) in &defaults {
            if !value.is_number() {
                continue;
            }
            assert!(
                NUMERIC_BOUNDS.iter().any(|(name, _, _)| name == key),
                "{key:?} is a numeric config key with no entry in NUMERIC_BOUNDS — config.set \
                 would take any value serde can coerce, including one that costs the machine its \
                 memory"
            );
        }

        // And nothing in the table names a key that no longer exists, which
        // would be a bound quietly guarding nothing.
        for (key, _, _) in NUMERIC_BOUNDS {
            assert!(
                defaults.contains_key(key),
                "NUMERIC_BOUNDS names {key:?}, which Config has not"
            );
        }
    }

    /// The refusal itself, at the state layer: it names the key, the value and
    /// the range, and it is an error rather than a panic. `dispatch.rs` covers
    /// the wire behaviour and the "wrote nothing" half.
    #[test]
    fn an_out_of_range_value_is_refused_with_the_range_in_the_message() {
        let daemon = idle("range", Config::default());
        let mut values = Map::new();
        values.insert("replay_seconds".to_string(), Value::from(u32::MAX));

        let error = match daemon.set_config(&values) {
            Ok(_) => panic!("a 136-year replay ring is not a setting"),
            Err(e) => format!("{e:#}"),
        };
        assert!(error.contains("replay_seconds"), "the error names the key: {error}");
        assert!(error.contains("4294967295"), "the error quotes the value: {error}");
        assert!(error.contains("600"), "the error states the accepted range: {error}");
    }

    /// Out-of-range is checked; wrong-*type* is still reported as the type
    /// error it is, because "must be 1 to 480" is a poor description of
    /// `"sixty"`. The two messages must not swap places.
    #[test]
    fn a_wrong_type_value_still_gets_the_type_error_not_a_range_error() {
        let daemon = idle("wrong-type", Config::default());
        let mut values = Map::new();
        values.insert("fps".to_string(), Value::from("sixty"));

        let error = match daemon.set_config(&values) {
            Ok(_) => panic!("fps is a number"),
            Err(e) => format!("{e:#}"),
        };
        assert!(error.contains("fps"), "the error names the key: {error}");
        assert!(
            error.contains("n’accepte pas"),
            "a string fps is a type problem, not a range problem: {error}"
        );
    }

    /// `clip` has to name the missing step rather than reporting some internal
    /// "no engine" condition — this string is what a user sees.
    #[test]
    fn clipping_while_idle_names_the_missing_step() {
        let daemon = idle("clip-idle", Config::default());
        let error = match daemon.clip() {
            Ok(_) => panic!("an idle daemon has no ring to clip from"),
            Err(e) => format!("{e:#}"),
        };
        assert_eq!(error, "le replay n’est pas activé — activez-le d’abord");
    }

    /// A clip that reaches the library must reach the clients too.
    ///
    /// This is the regression that shipped: `clip_saved` was broadcast by
    /// `dispatch::clip`, the socket command's handler, so a clip taken with
    /// the **hotkey** — which calls `Daemon::clip` directly from
    /// `window::handle_action` and never touches `dispatch` — was written to
    /// disk and announced to nobody. The desktop app's grid only showed it
    /// after a reload, while spec §10's stage-4 gate opens by requiring the
    /// opposite: "clip appears in the grid within a second of the hotkey".
    ///
    /// It survived three plans because the only way in was `Daemon::clip`,
    /// which needs an armed engine and a real capture ring and so is
    /// unreachable from any test. Asserting on `record_saved_clip` is what
    /// makes the guarantee testable at all.
    #[test]
    fn a_recorded_clip_is_announced_to_every_client() {
        use std::sync::mpsc::sync_channel;

        let daemon = idle("clip-saved-broadcast", Config::default());
        let (tx, rx) = sync_channel(crate::clients::OUTBOUND_QUEUE_DEPTH);
        daemon.clients.register(tx);

        daemon.record_saved_clip(&meta("20260803_120000"), PlaySound::Yes);

        // The library half — the part that already worked.
        assert_eq!(
            daemon.lock_library().first().map(|c| c.id.as_str()),
            Some("20260803_120000"),
            "a saved clip is prepended to the live library"
        );

        // The event half — the part that did not.
        let line = rx.try_recv().expect("a saved clip must broadcast clip_saved");
        let event: Event = serde_json::from_str(line.trim_end())
            .unwrap_or_else(|e| panic!("event line did not decode: {e}\nline: {line}"));
        assert_eq!(event.event, "clip_saved");
        assert_eq!(event.data.get("library_total").and_then(Value::as_u64), Some(1));
        assert_eq!(
            event.data.get("library_bytes").and_then(Value::as_u64),
            Some(meta("20260803_120000").bytes)
        );
        assert_eq!(event.data.get("removed_ids"), Some(&serde_json::json!([])));
        assert_eq!(
            event.data.get("id").and_then(Value::as_str),
            Some("20260803_120000"),
            "the event carries the ClipMeta the grid prepends"
        );
    }

    #[test]
    fn saved_clip_event_reports_automatic_pruning_and_remaining_bytes() {
        let (daemon, dir) = fixture("clip-prune-broadcast");
        daemon.lock_config().max_library_gb = 1;
        // Sizes come off the files: 700 MB each, allocated but never written.
        for clip in daemon.lock_library().iter_mut() {
            clip.bytes = 700_000_000;
            std::fs::File::options().write(true).open(path_of(&daemon, &clip.id)).unwrap().set_len(700_000_000).unwrap();
        }
        let saved = meta("20260726_130000");
        let path = dir.join("new.mp4");
        std::fs::write(&path, b"video").unwrap();
        daemon.store.insert(&saved, &path).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(crate::clients::OUTBOUND_QUEUE_DEPTH);
        daemon.clients.register(tx);

        daemon.record_saved_clip(&saved, PlaySound::No);

        let event: Event = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            event.data["removed_ids"],
            serde_json::json!(["20260726_100000", "20260726_110000"])
        );
        assert_eq!(event.data["library_total"], 2);
        assert_eq!(event.data["library_bytes"], 700_000_005_u64);
        assert_eq!(daemon.list(0, 200).total_bytes, 700_000_005);
        cleanup(&dir);
    }

    /// The sound must not be able to break the announcement. This drives the same
    /// function the hotkey reaches, with the sound turned on, and asserts the
    /// event still arrives -- the ordering the comment in `record_saved_clip`
    /// promises is otherwise only a comment.
    ///
    /// `idle` supplies the scratch `clip_dir`, and `config_path` is `None`, so
    /// nothing here can reach the developer's real config or clip folder.
    #[test]
    fn a_clip_is_still_announced_when_the_sound_is_on() {
        use std::sync::mpsc::sync_channel;

        let daemon = idle("clip-sound-on", Config { clip_sound: true, ..Config::default() });
        let (tx, rx) = sync_channel(crate::clients::OUTBOUND_QUEUE_DEPTH);
        daemon.clients.register(tx);

        daemon.record_saved_clip(&meta("20260817_120000"), PlaySound::Yes);

        let line = rx.try_recv().expect("a saved clip must still broadcast clip_saved");
        assert!(line.contains("clip_saved"), "the sound must not displace the event: {line}");
    }

    fn shot_meta(id: &str) -> ShotMeta {
        ShotMeta {
            id: id.to_string(),
            created: "2026-08-27T14:30:12".into(),
            bytes: 5,
            width: 1920,
            height: 1080,
        }
    }

    /// A screenshot that reaches disk must reach the clients too -- the same
    /// regression `a_recorded_clip_is_announced_to_every_client` guards
    /// against, for the sibling feature. `Daemon::screenshot` needs an armed
    /// engine and a real capture session, which is unreachable from this
    /// suite; asserting on `record_saved_shot` is what makes the guarantee
    /// testable ahead of Task 7's hotkey, which will call
    /// `Daemon::screenshot` directly the same way the clip hotkey calls
    /// `Daemon::clip`.
    #[test]
    fn a_recorded_shot_is_announced_to_every_client() {
        use std::sync::mpsc::sync_channel;

        let daemon = idle("shot-saved-broadcast", Config::default());
        let (tx, rx) = sync_channel(crate::clients::OUTBOUND_QUEUE_DEPTH);
        daemon.clients.register(tx);

        daemon.record_saved_shot(&shot_meta("20260827_143012"));

        let line = rx.try_recv().expect("a saved screenshot must broadcast shot_saved");
        let event: Event = serde_json::from_str(line.trim_end())
            .unwrap_or_else(|e| panic!("event line did not decode: {e}\nline: {line}"));
        assert_eq!(event.event, "shot_saved");
        assert_eq!(
            event.data.get("id").and_then(Value::as_str),
            Some("20260827_143012"),
            "the event carries the ShotMeta the Screenshots tab binds to"
        );
    }

    /// Writes one clip into `dir` and returns its id.
    fn plant(dir: &Path, id: &str) -> String {
        std::fs::create_dir_all(dir).expect("clip directory");
        std::fs::write(clippy_core::library::mp4_path(dir, id), b"video").expect("mp4");
        clippy_core::library::write_sidecar(dir, &meta(id)).expect("sidecar");
        id.to_string()
    }

    /// The cache `library.list` answers from was scanned against the old
    /// folder. Moving `clip_dir` invalidates every entry at once, so the move
    /// has to rebuild it — otherwise the grid keeps listing clips that are not
    /// in the configured folder any more, and every thumbnail points nowhere.
    #[test]
    fn moving_the_clips_folder_relists_what_is_in_the_new_one() {
        let (daemon, dir) = writable("clip-dir-move");
        let before = plant(&dir.join("clips"), "20260726_100000");
        daemon.rescan_library().expect("the starting folder scans");
        assert_eq!(
            daemon.list(0, 50).clips.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            vec![before],
            "the fixture starts with the clip in the configured folder"
        );

        let moved = dir.join("moved");
        let after = plant(&moved, "20260726_120000");
        let mut values = Map::new();
        values.insert("clip_dir".into(), Value::from(moved.to_string_lossy().as_ref()));
        daemon.set_config(&values).expect("a writable folder is accepted");

        let page = daemon.list(0, 50);
        assert_eq!(
            page.clips.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            vec![after],
            "the move must leave the library listing the new folder, not the old one"
        );
        assert_eq!(page.total, 1, "the old folder's clip must not be counted twice");
        cleanup(&dir);
    }

    /// The other half: a `clip_dir` that resolves to where it already pointed
    /// is not a move, and must not throw the cache away. `record_saved_clip`
    /// keeps that cache current between scans, so a needless rescan is not
    /// merely wasted I/O on a big library — it is the one place a clip written
    /// but not yet flushed could disappear from the list.
    #[test]
    fn setting_the_clips_folder_to_where_it_already_is_keeps_the_cache() {
        let (daemon, dir) = writable("clip-dir-same");
        let clips = dir.join("clips");
        std::fs::create_dir_all(&clips).expect("clip directory");
        // Only in the cache, never on disk: a rescan would drop it, and that is
        // exactly what this test is watching for.
        daemon.lock_library().push(meta("20260726_100000"));

        let mut values = Map::new();
        values.insert("clip_dir".into(), Value::from(clips.to_string_lossy().as_ref()));
        daemon.set_config(&values).expect("the folder it is already using is acceptable");

        assert_eq!(
            daemon.list(0, 50).total,
            1,
            "setting clip_dir to its current value must not rescan"
        );
        cleanup(&dir);
    }

    /// One `config.set` call carrying a single key.
    fn one(key: &str, value: u64) -> Map<String, Value> {
        let mut values = Map::new();
        values.insert(key.into(), Value::from(value));
        values
    }

    #[test]
    fn a_level_within_the_range_is_accepted() {
        let (daemon, dir) = writable("accepted");
        let update = daemon.set_config(&one("mic_volume", 40)).expect("40 is in range");
        assert_eq!(update.accepted.get("mic_volume"), Some(&Value::from(40)));
        cleanup(&dir);
    }

    #[test]
    fn a_level_above_the_range_is_refused() {
        let (daemon, dir) = writable("refused");
        let over = u64::from(MAX_VOLUME_PERCENT) + 1;
        let error = daemon
            .set_config(&one("mic_volume", over))
            .expect_err("one past the ceiling is out of range");
        assert!(format!("{error}").contains("de 0 à 200"), "unexpected message: {error}");
        cleanup(&dir);
    }

    /// The ceiling moved from 100 to 200, and a bounds table that did not move
    /// with it would refuse every boosted level the Settings slider can now
    /// reach — with the slider still happily offering them.
    /// `auto_arm` is off unless it was asked for, and turning it on does not
    /// arm anything now.
    ///
    /// The second half is the part worth pinning. `main` reads this key once,
    /// at startup, so a `config.set` that flipped it must leave the daemon
    /// exactly as idle as it found it — a settings toggle that seized the
    /// encoder the moment it was clicked would be answering a question about
    /// *next* launch by acting on this one.
    #[test]
    fn auto_arm_is_opt_in_and_does_not_arm_on_the_spot() {
        let (daemon, dir) = writable("auto-arm");
        assert!(!daemon.auto_arm(), "auto_arm must default to off");

        let mut values = Map::new();
        values.insert("auto_arm".into(), Value::Bool(true));
        let update = daemon.set_config(&values).expect("auto_arm is a known key");

        assert_eq!(update.accepted.get("auto_arm"), Some(&Value::Bool(true)));
        assert!(daemon.auto_arm(), "the daemon must read back what was saved");
        assert!(update.requires_rearm.is_empty(), "auto_arm has nothing to do with a session");
        assert!(!daemon.status().armed, "setting auto_arm must not arm the daemon now");
        cleanup(&dir);
    }

    #[test]
    fn a_boosted_level_is_accepted() {
        let (daemon, dir) = writable("boost");
        let update = daemon
            .set_config(&one("mic_volume", u64::from(MAX_VOLUME_PERCENT)))
            .expect("the documented maximum is in range");
        assert_eq!(update.accepted.get("mic_volume"), Some(&Value::from(MAX_VOLUME_PERCENT)));
        assert!(update.requires_rearm.is_empty(), "a boost asked for a re-arm");
        cleanup(&dir);
    }

    /// How frames come off the screen is chosen when the session opens: the
    /// sink's converter and encoder are built on the backend's own D3D11
    /// device, and a texture cannot cross devices. Without the re-arm the
    /// dropdown would move, the file would save, and the running capture would
    /// carry on using the backend the user just switched away from.
    #[test]
    fn switching_capture_method_needs_a_rearm() {
        let (daemon, dir) = writable("capture-method");
        let mut values = Map::new();
        values.insert("capture_method".into(), Value::from("dd"));
        let update = daemon.set_config(&values).expect("dd is one of the offered values");
        assert_eq!(update.requires_rearm, vec!["capture_method".to_string()]);
        cleanup(&dir);
    }

    #[test]
    fn changing_a_level_within_the_range_needs_no_rearm() {
        // Re-arming destroys the replay ring. A volume slider that costs the
        // user their last fifteen seconds on every nudge is a broken slider.
        let (daemon, dir) = writable("no-rearm");
        let update = daemon.set_config(&one("mic_volume", 60)).expect("60 is in range");
        assert!(update.requires_rearm.is_empty(), "a mid-range nudge asked for a re-arm");
        cleanup(&dir);
    }

    #[test]
    fn muting_a_source_needs_a_rearm() {
        // 0 closes a real capture stream, which only happens at arm time.
        let (daemon, dir) = writable("mute");
        let update = daemon.set_config(&one("mic_volume", 0)).expect("0 is in range");
        assert_eq!(update.requires_rearm, vec!["mic_volume".to_string()]);
        cleanup(&dir);
    }

    #[test]
    fn unmuting_a_source_needs_a_rearm() {
        let (daemon, dir) = writable("unmute");
        daemon.set_config(&one("system_volume", 0)).expect("0 is in range");
        let update = daemon.set_config(&one("system_volume", 80)).expect("80 is in range");
        assert_eq!(update.requires_rearm, vec!["system_volume".to_string()]);
        cleanup(&dir);
    }

    #[test]
    fn an_accepted_level_reaches_the_shared_gains() {
        // Without this the slider would move, the file would save, and
        // nothing the capture session reads would change.
        let (daemon, dir) = writable("applied");
        daemon.set_config(&one("mic_volume", 25)).expect("25 is in range");
        assert_eq!(daemon.gains.mic_percent(), 25);
        cleanup(&dir);
    }

    #[test]
    fn a_refused_change_leaves_the_shared_gains_alone() {
        // config.set is all-or-nothing: a rejected request must not have
        // applied a level the caller was told did not land.
        let (daemon, dir) = writable("refused-gains");
        daemon.set_config(&one("mic_volume", 999)).expect_err("999 is out of range");
        assert_eq!(daemon.gains.mic_percent(), 100);
        cleanup(&dir);
    }

    #[test]
    fn setting_one_level_leaves_the_other_alone() {
        // `gains.set` takes both percentages at once, so it reads them out of
        // the committed config rather than the request. Reading them from the
        // request instead would look correct in every single-key test and
        // silently zero the untouched source the moment a user moved one
        // slider -- the failure would be a mic that went dead when you turned
        // the game down.
        let (daemon, dir) = writable("one-at-a-time");
        daemon.set_config(&one("mic_volume", 30)).expect("30 is in range");
        assert_eq!(daemon.gains.mic_percent(), 30);
        assert_eq!(daemon.gains.system_percent(), 100, "the untouched level keeps its value");

        daemon.set_config(&one("system_volume", 40)).expect("40 is in range");
        assert_eq!(daemon.gains.system_percent(), 40);
        assert_eq!(daemon.gains.mic_percent(), 30, "the level set a moment ago survives");
        cleanup(&dir);
    }

    /// The gate must refuse before anything is written, like every other gate in
    /// `set_config`. A user who picks a text file must end up with the sound they
    /// had, not with a config pointing at something that will never play.
    #[test]
    fn a_file_that_is_not_audio_is_refused_and_changes_nothing() {
        let (daemon, config_path, dir) = with_scratch_config("sound-refused");
        let bogus = dir.join("not-audio.mp3");
        std::fs::write(&bogus, b"this is text, not audio").unwrap();

        // A real write first, so the "unchanged" assertion below compares a
        // file that exists against itself rather than against absence.
        // Without this, the decode gate moving to *after* `updated.save_to`
        // would still leave the file simply absent both before and after --
        // the in-memory write happens later in the function regardless of
        // where the gate sits, and the cache write is never reached on this
        // refusal path either way, so only a real on-disk comparison catches
        // the gate moving. Mirrors
        // `config_set_refuses_an_out_of_range_value_for_every_bounded_key` in
        // dispatch.rs.
        daemon.set_config(&one("fps", 30)).expect("the seed write must land");
        let before = std::fs::read(&config_path).unwrap();

        let mut values = Map::new();
        values.insert("clip_sound_path".into(), Value::from(bogus.to_string_lossy().as_ref()));
        let refused = daemon.set_config(&values);

        assert!(refused.is_err(), "a text file must not be accepted as a sound");
        let message = format!("{:#}", refused.unwrap_err());
        assert!(message.contains("not-audio.mp3"), "the error must name the file: {message}");
        assert_eq!(daemon.lock_config().clip_sound_path, "", "the old value must still stand");
        assert!(
            !Config::sound_cache_path(&config_path).exists(),
            "a refused sound must not leave a cache behind"
        );
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            before,
            "a refused clip_sound_path must leave the file on disk byte-unchanged -- this is \
             what catches the decode gate moving to after the save"
        );

        cleanup(&dir);
    }

    /// Clearing the key is "Reset to default", and the cache has to go with it --
    /// otherwise the next play finds a stale file beside a config that says
    /// "built-in" and plays the sound the user just removed. The sidecar has
    /// to go with it too (Fix 1): left behind, it would still name a sound
    /// nothing points at any more, which is not itself a bug today -- an
    /// empty `clip_sound_path` never looks at the cache or the sidecar at
    /// all -- but it is exactly the kind of leftover this pairing exists to
    /// not require anyone to reason about case by case.
    #[test]
    fn clearing_the_path_deletes_the_cache() {
        let (daemon, config_path, dir) = with_scratch_config("sound-reset");
        let cache = Config::sound_cache_path(&config_path);
        let sidecar = Config::sound_src_path(&config_path);
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, b"RIFF____WAVEstale").unwrap();
        std::fs::write(&sidecar, r"C:\previous\sound.mp3").unwrap();

        let mut values = Map::new();
        values.insert("clip_sound_path".into(), Value::from(""));
        daemon.set_config(&values).expect("clearing the sound must be accepted");

        assert!(!cache.exists(), "Reset must remove the converted copy");
        assert!(!sidecar.exists(), "Reset must remove the sidecar along with the cache");

        cleanup(&dir);
    }

    /// A missing path is refused by name rather than by codec error, because that
    /// is the mistake people actually make.
    #[test]
    fn a_path_that_does_not_exist_is_refused() {
        let (daemon, _config_path, dir) = with_scratch_config("sound-missing");
        let mut values = Map::new();
        values.insert("clip_sound_path".into(), Value::from(r"Z:\nope\ghost.mp3"));

        let message = format!("{:#}", daemon.set_config(&values).unwrap_err());
        assert!(message.contains("ghost.mp3"), "the error must name the file: {message}");

        cleanup(&dir);
    }

    /// Finding 1's fix, proven against a real `write_sound_cache` failure
    /// rather than only reasoned about: even though the write fails, a stale
    /// cache from a *previous* sound must not survive it. Before the sidecar
    /// existed, `repair_sound_cache`'s only guard was "does a cache file
    /// exist" -- one left behind here would have been trusted as current
    /// forever, and the daemon would keep playing sound A while `config.get`
    /// reports sound B. The sidecar this test also checks for is Fix 1's
    /// belt-and-suspenders on top of that: even if this compensating delete
    /// of the cache itself were ever skipped, a sidecar still naming the old
    /// sound would go stale the moment `clip_sound_path` changed, and
    /// `needs_sound_cache_rebuild` would catch the mismatch on its own.
    ///
    /// The failure is injected by pre-creating `clip-sound.wav.tmp` as a
    /// directory, so `write_sound_cache`'s own write to that path fails
    /// without ever touching the final cache path -- the closest a test gets
    /// to "disk full" or "antivirus lock" without either, and it leaves the
    /// stale cache itself unlocked, so a passing test proves the delete
    /// actually ran rather than merely that both sides failed together.
    ///
    /// Needs Media Foundation to decode the fixture sound, so this is
    /// `#[ignore]`d like every other test in the codebase that exercises a
    /// real decode -- see `clippy_core::sound::decode`'s
    /// `a_real_file_decodes_to_a_capped_wav`.
    #[test]
    #[ignore = "needs Media Foundation; run by hand with --ignored"]
    fn a_failed_cache_write_does_not_strand_a_stale_cache() {
        let (daemon, config_path, dir) = with_scratch_config("sound-write-fail");

        let sound = dir.join("chosen.wav");
        let pcm = vec![0u8; 4 * 44_100]; // one second of silence, real WAV bytes
        std::fs::write(&sound, clippy_core::sound::wav_from_pcm(&pcm, 2, 44_100, 16)).unwrap();

        let cache = Config::sound_cache_path(&config_path);
        let sidecar = Config::sound_src_path(&config_path);
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, b"RIFF____WAVEstale-audio-from-a-previous-sound").unwrap();
        std::fs::write(&sidecar, r"C:\previous\sound.mp3").unwrap();
        // Forces the write inside `write_sound_cache` to fail without
        // touching `cache` at all, so the fix's best-effort delete of
        // `cache` is not fighting the same failure that broke the write.
        let temp = cache.with_extension("wav.tmp");
        std::fs::create_dir_all(&temp).unwrap();

        let mut values = Map::new();
        values.insert("clip_sound_path".into(), Value::from(sound.to_string_lossy().as_ref()));
        daemon.set_config(&values).expect("decode succeeds even though the cache write fails");

        assert!(
            !cache.exists(),
            "a failed cache write must not strand the previous sound's cache -- the next \
             startup has to see it missing and rebuild, not trust a file that names the \
             wrong sound"
        );
        assert!(
            !sidecar.exists(),
            "the sidecar must go with the cache it describes, so a survivor here could never \
             be misread as still matching a cache that is gone"
        );

        cleanup(&dir);
    }

    /// `write_sound_cache` itself, not through `set_config` -- no decode, so
    /// no Media Foundation needed to prove the pairing Fix 1 depends on.
    #[test]
    fn write_sound_cache_writes_the_sidecar_beside_it() {
        let (_daemon, config_path, dir) = with_scratch_config("sound-write-sidecar");
        let wav = clippy_core::sound::wav_from_pcm(&[0u8; 8], 2, 44_100, 16);

        write_sound_cache(&config_path, &wav, r"C:\sounds\airhorn.mp3").unwrap();

        assert_eq!(std::fs::read(Config::sound_cache_path(&config_path)).unwrap(), wav);
        assert_eq!(
            std::fs::read_to_string(Config::sound_src_path(&config_path)).unwrap(),
            r"C:\sounds\airhorn.mp3",
            "the sidecar must name exactly the source `write_sound_cache` was given"
        );

        cleanup(&dir);
    }

    /// The review fix this closes: a sidecar write failing must not throw
    /// away a cache that landed correctly. Before this, `write_sound_cache`
    /// returned `Err` for *any* failure past the rename, including this one,
    /// and `set_config`'s caller treated every `Err` alike -- deleting the
    /// cache it had just correctly written, because it could not tell "the
    /// cache never landed" from "the cache landed but the bookkeeping file
    /// beside it didn't."
    ///
    /// The failure is injected by creating a directory at the sidecar's own
    /// path -- the same trick `a_failed_cache_write_does_not_strand_a_stale_cache`
    /// uses one file over, on `clip-sound.wav.tmp` -- so only the sidecar
    /// write fails; the cache's temp-then-rename has already landed by the
    /// time it runs.
    ///
    /// No decode involved, so no Media Foundation needed -- straight at
    /// `write_sound_cache`, like its neighbour above.
    #[test]
    fn write_sound_cache_survives_a_failed_sidecar_write() {
        let (_daemon, config_path, dir) = with_scratch_config("sound-sidecar-fail");
        let wav = clippy_core::sound::wav_from_pcm(&[0u8; 8], 2, 44_100, 16);

        let cache = Config::sound_cache_path(&config_path);
        let sidecar = Config::sound_src_path(&config_path);
        // `with_scratch_config` already created `dir`, the parent both paths
        // share, so this is the only setup the injected failure needs.
        std::fs::create_dir_all(&sidecar).unwrap();

        write_sound_cache(&config_path, &wav, r"C:\sounds\airhorn.mp3").expect(
            "a sidecar write failing must not fail the whole call -- the cache it describes \
             already landed",
        );

        assert_eq!(
            std::fs::read(&cache).unwrap(),
            wav,
            "the newly converted sound must survive a sidecar failure, not be discarded over a \
             bookkeeping file"
        );
        assert!(
            needs_sound_cache_rebuild(
                r"C:\sounds\airhorn.mp3",
                cache.exists(),
                std::fs::read_to_string(&sidecar).ok().as_deref()
            ),
            "an unreadable sidecar must still read as needing a rebuild -- that is the healing \
             this fix relies on instead of deleting the cache on the spot"
        );

        cleanup(&dir);
    }

    /// The same failure as `write_sound_cache_survives_a_failed_sidecar_write`,
    /// but end to end through `set_config`: proves Settings actually keeps
    /// the newly chosen sound in place, not only that the pure function
    /// returns `Ok` in isolation.
    ///
    /// Needs Media Foundation to decode the fixture sound, so this is
    /// `#[ignore]`d like every other test in the codebase that exercises a
    /// real decode -- see `clippy_core::sound::decode`'s
    /// `a_real_file_decodes_to_a_capped_wav`.
    #[test]
    #[ignore = "needs Media Foundation; run by hand with --ignored"]
    fn a_failed_sidecar_write_still_keeps_the_new_sound_through_set_config() {
        let (daemon, config_path, dir) = with_scratch_config("sound-sidecar-fail-e2e");

        let sound = dir.join("chosen.wav");
        let pcm = vec![0u8; 4 * 44_100]; // one second of silence, real WAV bytes
        std::fs::write(&sound, clippy_core::sound::wav_from_pcm(&pcm, 2, 44_100, 16)).unwrap();

        let cache = Config::sound_cache_path(&config_path);
        let sidecar = Config::sound_src_path(&config_path);
        // Same trick as the pure-function test above, applied through
        // `set_config` this time: the rename inside `write_sound_cache` has
        // already published the new cache by the time this write is tried.
        std::fs::create_dir_all(&sidecar).unwrap();

        let mut values = Map::new();
        values.insert("clip_sound_path".into(), Value::from(sound.to_string_lossy().as_ref()));
        daemon.set_config(&values).expect(
            "a sidecar write failure must not surface as a config.set error -- the cache it was \
             supposed to describe already landed",
        );

        assert!(
            cache.exists(),
            "the newly converted sound must survive a sidecar failure -- `set_config` must not \
             delete a cache that was never stale"
        );
        assert_eq!(
            std::fs::read(&cache).unwrap(),
            clippy_core::sound::decode::to_wav(&sound).unwrap(),
            "the surviving cache must hold the newly chosen sound, not a leftover from before \
             this call"
        );

        cleanup(&dir);
    }

    /// [`needs_sound_cache_rebuild`] with nothing configured: `resolve_sound`
    /// never looks at the cache when `clip_sound_path` is empty, so there is
    /// nothing for a rebuild to fix, however stale the files sitting there
    /// might be.
    #[test]
    fn needs_sound_cache_rebuild_is_false_with_nothing_configured() {
        assert!(!needs_sound_cache_rebuild("", false, None));
        assert!(!needs_sound_cache_rebuild("", true, Some("anything")));
    }

    #[test]
    fn needs_sound_cache_rebuild_is_true_when_the_cache_file_is_missing() {
        assert!(needs_sound_cache_rebuild(r"C:\sounds\airhorn.mp3", false, None));
    }

    /// A cache with no sidecar at all cannot be trusted, even though it
    /// exists -- exactly the gap the old "does a cache file exist" guard
    /// left open.
    #[test]
    fn needs_sound_cache_rebuild_is_true_when_the_sidecar_is_missing() {
        assert!(needs_sound_cache_rebuild(r"C:\sounds\airhorn.mp3", true, None));
    }

    #[test]
    fn needs_sound_cache_rebuild_is_false_when_the_sidecar_matches() {
        assert!(!needs_sound_cache_rebuild(
            r"C:\sounds\airhorn.mp3",
            true,
            Some(r"C:\sounds\airhorn.mp3")
        ));
    }

    /// Fix 1's headline case, and the whole reason the sidecar exists: pick
    /// `airhorn.mp3`, quit, hand-edit `clip_sound_path` in `config.toml` to
    /// `boom.mp3`, restart. The old guard (`chosen.is_empty() ||
    /// cache.exists()`) saw a cache and stopped there, so every clip kept
    /// playing `airhorn` forever while `config.get` reported `boom`.
    ///
    /// This builds exactly that on-disk state -- a cache and a sidecar both
    /// naming `airhorn.mp3` -- and reads the sidecar back the same way
    /// `repair_sound_cache` does, rather than handing
    /// `needs_sound_cache_rebuild` a string the test made up, so a future
    /// change to how the sidecar is read cannot drift silently away from
    /// what this test proves. It stops at the decision rather than a real
    /// decode, which needs Media Foundation -- see this module's other
    /// `#[ignore]`d sound tests for that half.
    #[test]
    fn a_hand_edited_config_naming_a_different_sound_is_detected_as_needing_rebuild() {
        let (_daemon, config_path, dir) = with_scratch_config("sound-hand-edited");
        let cache = Config::sound_cache_path(&config_path);
        let sidecar = Config::sound_src_path(&config_path);
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, b"RIFF____WAVEairhorn").unwrap();
        std::fs::write(&sidecar, r"C:\sounds\airhorn.mp3").unwrap();

        let on_disk_sidecar = std::fs::read_to_string(&sidecar).ok();
        assert!(
            needs_sound_cache_rebuild(
                r"C:\sounds\boom.mp3",
                cache.exists(),
                on_disk_sidecar.as_deref()
            ),
            "a config hand-edited to name a different sound than the cache was built from must \
             be rebuilt, not trusted just because a cache file happens to exist"
        );
        assert!(
            !needs_sound_cache_rebuild(
                r"C:\sounds\airhorn.mp3",
                cache.exists(),
                on_disk_sidecar.as_deref()
            ),
            "an unedited config must not trigger a needless rebuild"
        );

        cleanup(&dir);
    }
}
