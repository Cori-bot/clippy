//! Automatic clips on in-game moments.
//!
//! Each game reports through its own *official* local channel, never by
//! reading the game's memory:
//! - Counter-Strike 2: Valve's Game State Integration, which POSTs JSON to a
//!   loopback HTTP listener named in a `gamestate_integration_*.cfg`.
//! - League of Legends: Riot's Live Client Data API on `127.0.0.1:2999`.
//!
//! Moments close together become one clip ([`Burst`]): a triple kill is one
//! clip titled "Triple kill", not three overlapping ones.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use serde_json::Value;

use crate::state::Daemon;

/// Something worth keeping that just happened in a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub game: &'static str,
    pub kills: u32,
    /// A named feat ("Ace", "Penta kill") that outranks the kill count.
    pub feat: Option<&'static str>,
}

/// Moments gathered into the clip they will be saved as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Burst {
    pub game: &'static str,
    pub kills: u32,
    pub feat: Option<&'static str>,
    first: Instant,
    deadline: Instant,
}

impl Burst {
    /// "Triple kill · Counter-Strike 2".
    pub fn title(&self) -> String {
        let what = self.feat.unwrap_or(match self.kills {
            0 | 1 => "Kill",
            2 => "Double kill",
            3 => "Triple kill",
            4 => "Quadra kill",
            _ => "Multi kill",
        });
        format!("{what} · {}", self.game)
    }
}

/// Holds a burst open while moments keep coming, then hands it over.
#[derive(Debug)]
pub struct Debouncer {
    pending: Option<Burst>,
    /// How long after the last moment the clip is saved: the aftermath
    /// (the kill feed, the reaction) belongs in it.
    after: Duration,
    /// Longest a burst may stay open, counted from its first moment, so that
    /// moment is still inside the replay buffer when the clip is cut.
    span: Duration,
}

impl Debouncer {
    pub fn new(after: Duration, span: Duration) -> Self {
        Self { pending: None, after, span: span.max(after) }
    }

    pub fn push(&mut self, moment: Moment, now: Instant) {
        match &mut self.pending {
            Some(burst) if burst.game == moment.game => {
                burst.kills += moment.kills;
                burst.feat = moment.feat.or(burst.feat);
                burst.deadline = (now + self.after).min(burst.first + self.span);
            }
            _ => {
                self.pending = Some(Burst {
                    game: moment.game,
                    kills: moment.kills,
                    feat: moment.feat,
                    first: now,
                    deadline: now + self.after,
                });
            }
        }
    }

    /// The burst to save now, if its deadline has passed.
    pub fn due(&mut self, now: Instant) -> Option<Burst> {
        if self.pending.as_ref().is_some_and(|b| now >= b.deadline) {
            self.pending.take()
        } else {
            None
        }
    }
}

/// A moment and when its source saw it: a long clip save must not push the
/// moments queued behind it later in time.
type Stamped = (Moment, Instant);

/// Seconds of aftermath kept after the last moment of a burst.
const AFTERMATH: Duration = Duration::from_secs(3);
/// Context kept before the first moment, at least.
const LEAD: Duration = Duration::from_secs(5);

/// Starts the auto-clip service: the moment sources and the thread that
/// turns bursts into saved clips. Every source checks the setting itself, so
/// turning it off in settings takes effect without a restart.
pub fn spawn(daemon: Arc<Daemon>) -> Result<()> {
    let (tx, rx) = channel();
    let weak = Arc::downgrade(&daemon);
    std::thread::Builder::new().name("clippy-autoclip".into()).spawn(move || saver(weak, rx))?;
    match TcpListener::bind(("127.0.0.1", CS2_PORT)) {
        Ok(listener) => {
            let (weak, tx) = (Arc::downgrade(&daemon), tx.clone());
            std::thread::Builder::new()
                .name("clippy-cs2-gsi".into())
                .spawn(move || serve_cs2(listener, weak, tx))?;
        }
        Err(error) => tracing::warn!(%error, port = CS2_PORT, "CS2 game state listener unavailable"),
    }
    let (weak, lol_tx) = (Arc::downgrade(&daemon), tx.clone());
    std::thread::Builder::new()
        .name("clippy-lol-events".into())
        .spawn(move || poll_lol(weak, lol_tx))?;
    // Its own thread: Steam libraries can sit on slow or missing drives.
    let weak = Arc::downgrade(&daemon);
    std::thread::Builder::new().name("clippy-cs2-config".into()).spawn(move || {
        // Kept current while the setting is on: a CS2 installed or moved
        // later still gets the config (read when CS2 starts).
        loop {
            match weak.upgrade() {
                None => return,
                // End-to-end tests drive a real daemon without touching the
                // developer's own CS2 install.
                Some(_) if std::env::var_os("CLIPPY_SKIP_CS2_CONFIG").is_some() => return,
                Some(daemon) if daemon.auto_clip_settings().enabled => {
                    drop(daemon);
                    let files = install_cs2_cfg();
                    tracing::debug!(?files, "CS2 game state config");
                    std::thread::sleep(Duration::from_secs(60));
                }
                Some(_) => std::thread::sleep(Duration::from_secs(2)),
            }
        }
    })?;
    let weak = Arc::downgrade(&daemon);
    std::thread::Builder::new()
        .name("clippy-valorant-feed".into())
        .spawn(move || watch_valorant(weak, tx))?;
    Ok(())
}

fn saver(daemon: Weak<Daemon>, moments: Receiver<Stamped>) {
    let mut debouncer: Option<Debouncer> = None;
    loop {
        let message = moments.recv_timeout(Duration::from_millis(250));
        let Some(daemon) = daemon.upgrade() else { return };
        let settings = daemon.auto_clip_settings();
        // The buffer length bounds how long a burst may stay open.
        let span = Duration::from_secs(u64::from(settings.replay_seconds)).saturating_sub(LEAD);
        let debouncer = debouncer.get_or_insert_with(|| Debouncer::new(AFTERMATH, span));
        debouncer.span = span.max(AFTERMATH);
        match message {
            Ok((moment, at)) if settings.enabled => debouncer.push(moment, at),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some(burst) = debouncer.due(Instant::now()) else { continue };
        if burst.kills < settings.min_kills && burst.feat.is_none() {
            continue;
        }
        match daemon.clip() {
            Ok(Some(meta)) => {
                match daemon.rename(&meta.id, &burst.title()) {
                    // Replaces the plain "Clip enregistré" banner the save
                    // just raised with the moment's name.
                    Ok(renamed) if daemon.lock_config().clip_banner => {
                        let mut message = crate::state::clip_banner_message(&renamed);
                        message.title = "Clip automatique".into();
                        crate::banner::show(message);
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "could not title an automatic clip"),
                }
                tracing::info!(title = burst.title(), "automatic clip saved");
            }
            Ok(None) => tracing::info!("automatic clip skipped: nothing buffered yet"),
            // Not armed, or the engine is rebuilding: the moment is gone.
            Err(error) => tracing::info!(%error, "automatic clip skipped"),
        }
    }
}

// ---------------------------------------------------------------- CS2 (GSI)

/// Loopback port the CS2 config points the game at.
pub const CS2_PORT: u16 = 52_417;
/// Shared with the config file: CS2 sends it back in every POST, which tells
/// our listener's traffic apart from anything else that finds the port.
const CS2_TOKEN: &str = "clippy-autoclip";
const CS2_GAME: &str = "Counter-Strike 2";
pub const CS2_CFG_NAME: &str = "gamestate_integration_clippy.cfg";

/// The `gamestate_integration_clippy.cfg` CS2 reads at launch.
pub fn cs2_cfg() -> String {
    format!(
        "\"Clippy\"\n{{\n\t\"uri\"\t\"http://127.0.0.1:{CS2_PORT}/\"\n\t\"timeout\"\t\"5.0\"\n\t\"buffer\"\t\"0.0\"\n\t\"throttle\"\t\"0.0\"\n\t\"heartbeat\"\t\"30.0\"\n\t\"auth\"\n\t{{\n\t\t\"token\"\t\"{CS2_TOKEN}\"\n\t}}\n\t\"data\"\n\t{{\n\t\t\"provider\"\t\"1\"\n\t\t\"map\"\t\"1\"\n\t\t\"round\"\t\"1\"\n\t\t\"player_id\"\t\"1\"\n\t\t\"player_state\"\t\"1\"\n\t}}\n}}\n"
    )
}

/// Follows the local player's kills across GSI posts.
#[derive(Debug, Default)]
pub struct Cs2Tracker {
    /// `round_kills` at the last post about the local player, once one has
    /// been seen: the first post only sets the baseline, so starting Clippy
    /// mid-round does not clip kills from before it was listening.
    round_kills: Option<u32>,
}

impl Cs2Tracker {
    pub fn update(&mut self, post: &Value) -> Option<Moment> {
        if post.pointer("/auth/token").and_then(Value::as_str) != Some(CS2_TOKEN) {
            return None;
        }
        let me = post.pointer("/provider/steamid").and_then(Value::as_str)?;
        // While dead or spectating, `player` is whoever is being watched.
        if post.pointer("/player/steamid").and_then(Value::as_str) != Some(me) {
            return None;
        }
        let now = post.pointer("/player/state/round_kills").and_then(Value::as_u64)? as u32;
        let before = self.round_kills.replace(now)?;
        // Lower means a new round (or map) began: only a baseline.
        let kills = now.checked_sub(before).filter(|&k| k > 0)?;
        // Casual and deathmatch go past five: the ace is the crossing, once.
        Some(Moment { game: CS2_GAME, kills, feat: (before < 5 && now >= 5).then_some("Ace") })
    }
}

fn serve_cs2(listener: TcpListener, daemon: Weak<Daemon>, moments: Sender<Stamped>) {
    let mut tracker = Cs2Tracker::default();
    let mut installed = false;
    for stream in listener.incoming() {
        let Some(daemon) = daemon.upgrade() else { return };
        let Ok(stream) = stream else {
            // Out of handles or similar: do not spin.
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let body = match read_post(stream) {
            Ok(body) => body,
            Err(error) => {
                tracing::debug!(%error, "unreadable game state post");
                continue;
            }
        };
        if !daemon.auto_clip_settings().enabled {
            // Re-armed later, it must start from a fresh baseline.
            tracker = Cs2Tracker::default();
            continue;
        }
        if !installed {
            installed = true;
            tracing::info!("CS2 game state integration connected");
        }
        let Ok(post) = serde_json::from_slice::<Value>(&body) else { continue };
        if let Some(moment) = tracker.update(&post) {
            if moments.send((moment, Instant::now())).is_err() {
                return;
            }
        }
    }
}

/// Reads one HTTP request's body and answers 200, which CS2 needs to keep
/// sending (a failed post makes it resend full state instead of deltas).
///
/// One request at a time, so a client must not be able to hold the loop:
/// the whole request has [`POST_DEADLINE`], lines and headers are bounded.
/// A browser page could reach this port too; browsers always send `Origin`
/// on a cross-site POST and CS2 never does, so such a request is refused.
fn read_post(stream: TcpStream) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(Deadline {
        stream: stream.try_clone()?,
        at: Instant::now() + POST_DEADLINE,
    });
    let mut length = 0usize;
    let mut line = String::new();
    for _ in 0..64 {
        line.clear();
        if (&mut reader).take(8 * 1024).read_line(&mut line)? == 0 || !line.ends_with('\n') {
            anyhow::bail!("header line cut short or too long");
        }
        let header = line.trim_end();
        if header.is_empty() {
            anyhow::ensure!(length <= 1 << 20, "game state post too large");
            let mut body = vec![0; length];
            let mut read = 0;
            while read < length {
                match reader.read(&mut body[read..])? {
                    0 => anyhow::bail!("connection closed in the body"),
                    n => read += n,
                }
            }
            let mut stream = stream;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok(body);
        }
        if let Some((name, value)) = header.split_once(':') {
            anyhow::ensure!(!name.eq_ignore_ascii_case("origin"), "browser request refused");
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().context("content-length")?;
            }
        }
    }
    anyhow::bail!("too many headers")
}

/// A socket whose every read shares one deadline, so trickling a byte at a
/// time cannot stretch a request past it.
struct Deadline {
    stream: TcpStream,
    at: Instant,
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf)
    }
}

/// Most a game state request may take, headers and body together.
const POST_DEADLINE: Duration = Duration::from_secs(3);

/// Every `cfg` folder of a CS2 install the Steam libraries list.
pub fn cs2_cfg_dirs() -> Vec<PathBuf> {
    let Some(steam) = steam_path() else { return Vec::new() };
    let vdf = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf"))
        .unwrap_or_default();
    let mut libraries = library_paths(&vdf);
    libraries.push(steam);
    // The registry and the vdf spell one folder differently (case, slashes).
    let mut dirs: Vec<PathBuf> = libraries
        .into_iter()
        .filter_map(|lib| {
            std::fs::canonicalize(
                lib.join(r"steamapps\common\Counter-Strike Global Offensive\game\csgo\cfg"),
            )
            .ok()
        })
        .filter(|dir| dir.is_dir())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// The `"path"` values of a `libraryfolders.vdf`, unescaped.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            (quoted.next()? == "path").then(|| quoted.next()).flatten()
        })
        .map(|path| PathBuf::from(path.replace("\\\\", "\\")))
        .collect()
}

/// Writes the CS2 config into every install found. Returns the files written
/// or already current. CS2 reads them when it starts.
pub fn install_cs2_cfg() -> Vec<PathBuf> {
    let content = cs2_cfg();
    cs2_cfg_dirs()
        .into_iter()
        .filter_map(|dir| {
            let file = dir.join(CS2_CFG_NAME);
            if std::fs::read_to_string(&file).ok().as_deref() == Some(content.as_str()) {
                return Some(file);
            }
            match std::fs::write(&file, &content) {
                Ok(()) => Some(file),
                Err(error) => {
                    tracing::warn!(%error, file = %file.display(), "could not write the CS2 config");
                    None
                }
            }
        })
        .collect()
}

fn steam_path() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len]));
    path.is_dir().then_some(path)
}

// ---------------------------------------------------- Valorant (kill feed)

const VALORANT_GAME: &str = "Valorant";

fn watch_valorant(daemon: Weak<Daemon>, moments: Sender<Stamped>) {
    use crate::valorant::{EXE, REGION, RowTracker, feed_rows};
    let mut watching = false;
    let mut rows = RowTracker::default();
    // End-to-end tests play recorded footage in a stand-in program.
    let exe = std::env::var("CLIPPY_VALORANT_EXE").unwrap_or_else(|_| EXE.to_owned());
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let Some(daemon) = daemon.upgrade() else { return };
        let in_game = daemon.auto_clip_settings().enabled
            && daemon.status().game.is_some_and(|game| game.eq_ignore_ascii_case(&exe));
        if !in_game {
            // Only a stop the engine took counts: otherwise retry next tick.
            if watching && daemon.probe(None).is_ok() {
                watching = false;
                rows = RowTracker::default();
            }
            continue;
        }
        watching = true;
        let frame = match daemon.probe(Some(REGION)) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                tracing::debug!(%error, "kill feed probe unavailable");
                continue;
            }
        };
        let reading =
            feed_rows(&frame.bgra, frame.width as usize, frame.height as usize, frame.stride);
        let new = rows.push(&reading.kills);
        let moment = Moment { game: VALORANT_GAME, kills: new, feat: None };
        if new > 0 && moments.send((moment, Instant::now())).is_err() {
            return;
        }
    }
}

// ------------------------------------------------- League of Legends (LCDA)

const LOL_GAME: &str = "League of Legends";
const LOL_API: &str = "https://127.0.0.1:2999/liveclientdata";

/// Follows the Live Client Data event list of one game.
#[derive(Debug, Default)]
pub struct LolTracker {
    /// Highest `EventID` handled. `None` until the first list of a game sets
    /// the baseline, so joining mid-game does not replay old kills.
    last: Option<u64>,
}

impl LolTracker {
    /// The moments among `events` newer than the last call, for the player
    /// named `me` (a Riot ID "Name#TAG"; events may carry only "Name").
    pub fn update(&mut self, me: &str, events: &[Value]) -> Vec<Moment> {
        let id = |e: &Value| e.get("EventID").and_then(Value::as_u64);
        let newest = events.iter().filter_map(id).max();
        let Some(last) = self.last else {
            self.last = Some(newest.unwrap_or(0));
            return Vec::new();
        };
        // A new game restarts the IDs.
        if newest.is_some_and(|n| n < last) {
            self.last = newest;
            return Vec::new();
        }
        self.last = newest.or(Some(last));
        let short = me.split('#').next().unwrap_or(me);
        let mine = |e: &Value, field: &str| {
            e.get(field).and_then(Value::as_str).is_some_and(|n| n == me || n == short)
        };
        let mut moments = Vec::new();
        for event in events.iter().filter(|e| id(e).is_some_and(|i| i > last)) {
            let moment = match event.get("EventName").and_then(Value::as_str) {
                Some("ChampionKill") if mine(event, "KillerName") => {
                    Moment { game: LOL_GAME, kills: 1, feat: None }
                }
                Some("Multikill") if mine(event, "KillerName") => Moment {
                    game: LOL_GAME,
                    kills: 0,
                    feat: match event.get("KillStreak").and_then(Value::as_u64) {
                        Some(2) => Some("Double kill"),
                        Some(3) => Some("Triple kill"),
                        Some(4) => Some("Quadra kill"),
                        Some(5..) => Some("Penta kill"),
                        _ => None,
                    },
                },
                Some("Ace") if mine(event, "Acer") => {
                    Moment { game: LOL_GAME, kills: 0, feat: Some("Ace") }
                }
                _ => continue,
            };
            moments.push(moment);
        }
        moments
    }
}

fn poll_lol(daemon: Weak<Daemon>, moments: Sender<Stamped>) {
    // The API serves a self-signed certificate on loopback only.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .disable_verification(true)
                .build(),
        )
        .timeout_global(Some(Duration::from_millis(800)))
        // Loopback only: an HTTPS_PROXY from the environment must not see it.
        .proxy(None)
        .build()
        .into();
    let get = |path: &str| -> Option<Value> {
        let mut response = agent.get(format!("{LOL_API}/{path}")).call().ok()?;
        serde_json::from_str(&response.body_mut().read_to_string().ok()?).ok()
    };
    let mut tracker = LolTracker::default();
    loop {
        let Some(daemon_now) = daemon.upgrade() else { return };
        let enabled = daemon_now.auto_clip_settings().enabled;
        drop(daemon_now);
        // No game: the port refuses at once, so idle polling costs nothing,
        // but there is no need to ask more often than a game can start.
        let in_game = enabled
            .then(|| get("activeplayername"))
            .flatten()
            .and_then(|v| v.as_str().map(str::to_owned));
        let Some(me) = in_game else {
            tracker = LolTracker::default();
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        if let Some(events) = get("eventdata") {
            let list = events.get("Events").and_then(Value::as_array).cloned().unwrap_or_default();
            for moment in tracker.update(&me, &list) {
                if moments.send((moment, Instant::now())).is_err() {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kill(game: &'static str) -> Moment {
        Moment { game, kills: 1, feat: None }
    }

    #[test]
    fn kills_close_together_become_one_clip_saved_after_the_last() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_secs(3), Duration::from_secs(10));
        d.push(kill(CS2_GAME), t0);
        d.push(kill(CS2_GAME), t0 + Duration::from_secs(2));
        assert_eq!(d.due(t0 + Duration::from_secs(4)), None, "the second kill reopened the wait");
        d.push(kill(CS2_GAME), t0 + Duration::from_secs(4));
        let burst = d.due(t0 + Duration::from_secs(7)).unwrap();
        assert_eq!(burst.kills, 3);
        assert_eq!(burst.title(), "Triple kill · Counter-Strike 2");
        assert_eq!(d.due(t0 + Duration::from_secs(8)), None);
    }

    #[test]
    fn a_long_burst_is_cut_before_its_first_kill_leaves_the_buffer() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_secs(3), Duration::from_secs(10));
        for s in 0..=9 {
            d.push(kill(CS2_GAME), t0 + Duration::from_secs(s));
        }
        assert!(d.due(t0 + Duration::from_secs(10)).is_some());
    }

    #[test]
    fn a_named_feat_titles_the_clip() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_secs(3), Duration::from_secs(10));
        d.push(kill(LOL_GAME), t0);
        d.push(Moment { game: LOL_GAME, kills: 0, feat: Some("Penta kill") }, t0);
        assert_eq!(d.due(t0 + Duration::from_secs(3)).unwrap().title(), "Penta kill · League of Legends");
    }

    fn post(me: &str, watched: &str, round_kills: u64) -> Value {
        json!({
            "auth": {"token": CS2_TOKEN},
            "provider": {"steamid": me},
            "player": {"steamid": watched, "state": {"round_kills": round_kills}},
        })
    }

    #[test]
    fn cs2_counts_only_the_local_players_new_kills() {
        let mut t = Cs2Tracker::default();
        assert_eq!(t.update(&post("1", "1", 2)), None, "first post is the baseline");
        assert_eq!(t.update(&post("1", "1", 3)).unwrap().kills, 1);
        assert_eq!(t.update(&post("1", "2", 4)), None, "spectating a teammate");
        assert_eq!(t.update(&post("1", "1", 0)), None, "new round");
        let ace = t.update(&post("1", "1", 5)).unwrap();
        assert_eq!((ace.kills, ace.feat), (5, Some("Ace")));
        let sixth = t.update(&post("1", "1", 6)).unwrap();
        assert_eq!((sixth.kills, sixth.feat), (1, None), "the ace is named once");
        let mut forged = post("1", "1", 9);
        forged["auth"]["token"] = json!("other");
        assert_eq!(t.update(&forged), None);
    }

    #[test]
    fn cs2_cfg_points_at_the_listener_with_the_token() {
        let cfg = cs2_cfg();
        assert!(cfg.contains(&format!("http://127.0.0.1:{CS2_PORT}/")));
        assert!(cfg.contains(CS2_TOKEN) && cfg.contains("\"player_state\"\t\"1\""));
    }

    #[test]
    fn library_paths_reads_and_unescapes_the_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"E:\\\\SteamLibrary\"\n\t}\n}";
        assert_eq!(
            library_paths(vdf),
            vec![PathBuf::from("C:\\Program Files (x86)\\Steam"), PathBuf::from("E:\\SteamLibrary")]
        );
    }

    #[test]
    fn cs2_listener_answers_200_and_hands_over_the_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}").unwrap();
            let mut answer = String::new();
            s.read_to_string(&mut answer).unwrap();
            answer
        });
        let (stream, _) = listener.accept().unwrap();
        assert_eq!(read_post(stream).unwrap(), b"{\"a\":1}");
        assert!(client.join().unwrap().starts_with("HTTP/1.1 200 OK"));
    }

    /// Opt-in, read-only: `cargo test -p clippy-daemon real_cs2 -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_cs2_install_is_found() {
        let dirs = cs2_cfg_dirs();
        println!("{dirs:?}");
        assert!(!dirs.is_empty());
    }

    fn post_from(request: &'static [u8], slow: bool) -> (Result<Vec<u8>>, Duration) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            if slow {
                // A byte every second never finishes the headers in time.
                for byte in request.iter().take(6) {
                    if s.write_all(&[*byte]).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            } else {
                let _ = s.write_all(request);
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let result = read_post(stream);
        // Timed before the join: the slow client is still trickling.
        let took = started.elapsed();
        let _ = client.join();
        (result, took)
    }

    #[test]
    fn cs2_listener_refuses_browser_posts_and_slow_clients() {
        let browser: &[u8] = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nContent-Length: 2\r\n\r\n{}";
        let (refused, _) = post_from(browser, false);
        assert!(format!("{:#}", refused.unwrap_err()).contains("browser"));
        let (slow, took) = post_from(b"POST / HTTP/1.1\r\n", true);
        assert!(slow.is_err());
        assert!(took < POST_DEADLINE + Duration::from_secs(1), "{took:?}");
    }

    fn ev(id: u64, name: &str, extra: Value) -> Value {
        let mut e = json!({"EventID": id, "EventName": name, "EventTime": 1.0});
        e.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        e
    }

    #[test]
    fn lol_reports_new_kills_and_feats_of_the_active_player_only() {
        let me = "Coco#EUW";
        let mut t = LolTracker::default();
        let old = vec![ev(0, "GameStart", json!({})), ev(1, "ChampionKill", json!({"KillerName": "Coco"}))];
        assert!(t.update(me, &old).is_empty(), "baseline");
        let mut now = old.clone();
        now.push(ev(2, "ChampionKill", json!({"KillerName": "Coco"})));
        now.push(ev(3, "ChampionKill", json!({"KillerName": "Other"})));
        now.push(ev(4, "Multikill", json!({"KillerName": "Coco#EUW", "KillStreak": 3})));
        now.push(ev(5, "Ace", json!({"Acer": "Other"})));
        let moments = t.update(me, &now);
        assert_eq!(moments.len(), 2);
        assert_eq!(moments[0].kills, 1);
        assert_eq!(moments[1].feat, Some("Triple kill"));
        assert!(t.update(me, &now).is_empty(), "nothing new");
        // A new game restarts the IDs: baseline again.
        assert!(t.update(me, &[ev(0, "GameStart", json!({}))]).is_empty());
        assert_eq!(t.update(me, &[ev(1, "ChampionKill", json!({"KillerName": "Coco"}))]).len(), 1);
    }
}
