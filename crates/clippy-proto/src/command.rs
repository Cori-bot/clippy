//! The typed command set, parsed out of a [`Request`]'s loose argument bag.

use serde_json::Value;

use crate::message::Request;

/// Page size when `library.list` omits `limit`.
pub const DEFAULT_LIST_LIMIT: usize = 50;
/// Hard ceiling on `library.list`'s page size.
pub const MAX_LIST_LIMIT: usize = 500;

/// One audio track an export keeps: its position among the clip's audio
/// streams and its level (0..=200, 100 unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportTrack {
    pub index: usize,
    pub gain: u32,
}

/// Audio layout of the exported MP4, independently of trim precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportOutput {
    Master,
    Share,
}

/// Every command the daemon answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status,
    Arm,
    Disarm,
    Clip,
    ConfigGet,
    ConfigSet(serde_json::Map<String, Value>),
    LibraryList {
        offset: usize,
        limit: usize,
    },
    LibraryDelete {
        clip_id: String,
    },
    LibraryRename {
        clip_id: String,
        title: String,
    },
    LibraryFavorite {
        clip_id: String,
        favorite: bool,
    },
    LibraryReveal {
        clip_id: String,
    },
    /// Puts the clip's file on the clipboard as a file (`CF_HDROP`), ready to
    /// paste into Discord, a chat window or Explorer.
    LibraryCopy {
        clip_id: String,
    },
    /// Every keyframe position in a clip, in milliseconds. The trim bar draws
    /// a tick at each one and snaps the in-point to them (spec §6.3).
    LibraryKeyframes {
        clip_id: String,
    },
    /// Real video presentation timestamps and exclusive end, in 100 ns ticks.
    LibraryFrames {
        clip_id: String,
    },
    /// Exports `start_ms..end_ms` of a clip as a **new clip in the library**,
    /// answering with its `ClipMeta`.
    ///
    /// No `dest`: the daemon allocates the id and the path, because an export
    /// that landed outside the clip directory would be invisible to every
    /// other `library.*` command. `fast` copies from a keyframe; `precise`
    /// reencodes the selected real frame boundaries.
    LibraryExport {
        clip_id: String,
        start_ms: u64,
        end_ms: u64,
        mode: String,
        /// `master` (default) retains editable stems; `share` writes only the mix.
        output: ExportOutput,
        /// The audio tracks to keep and how loud, when the export rebuilds the
        /// sound; `None` preserves the existing audio for a master, or selects
        /// its canonical first mix for a share. An empty list removes audio.
        tracks: Option<Vec<ExportTrack>>,
        /// Largest file the export may produce, in megabytes (10^6 bytes).
        /// Only for a precise share: the video bitrate and, when needed, the
        /// picture size are chosen so the file fits (Discord's upload caps).
        size_limit_mb: Option<u32>,
        /// Portrait 9:16 crop centred this many thousandths across the source
        /// (0..=1000, 500 = centre). Only for a precise export.
        vertical_center: Option<u16>,
    },
    LibraryExportStart {
        clip_id: String, start_ms: u64, end_ms: u64, mode: String,
        output: ExportOutput, tracks: Option<Vec<ExportTrack>>, size_limit_mb: Option<u32>,
        vertical_center: Option<u16>,
        request_token: Option<String>,
    },
    LibraryExportStatus { job_id: Option<String>, request_token: Option<String> },
    LibraryExportCancel { job_id: String },
    /// The clip's audio tracks as separate files the editor can play next to
    /// each other: `{tracks: [{index, name, file}]}`.
    LibraryTracks {
        clip_id: String,
    },
    /// The games the daemon knows: detected ones, added ones, and their audio
    /// profiles.
    GamesList,
    /// Edits one game, creating a manual entry when the program is unknown.
    /// `profile` is passed through as JSON and validated by the daemon.
    GamesSet {
        exe: String,
        enabled: Option<bool>,
        name: Option<String>,
        profile: Option<Value>,
    },
    GamesRemove {
        exe: String,
    },
    /// The windows open on this PC, for the "add a game" picker.
    WindowsList,
    /// Writes a screenshot from the live capture session.
    ///
    /// A command rather than hotkey-only wiring, so the tray, the desktop app and
    /// any third-party client reach it the same way — spec §3.2's rule that the
    /// UI holds no privilege the socket does not.
    Screenshot,
    ShotsList {
        offset: usize,
        limit: usize,
    },
    ShotsDelete {
        shot_id: String,
    },
    ShotsReveal {
        shot_id: String,
    },
    /// Puts an already-saved screenshot back on the clipboard.
    ///
    /// The daemon's job rather than the app's: `clippy-ui.exe` is not resident, so a
    /// copy implemented in the app would be unavailable in exactly the situation
    /// the Screenshots tab is open to serve.
    ShotsCopy {
        shot_id: String,
    },
    MonitorsList,
    EncodersList,
    /// The applications that have an audio session on this PC, playing or not,
    /// for the settings page's "record only these applications" picker.
    AudioApps,
    StatsSubscribe {
        enabled: bool,
    },
    /// Opens the "change clips folder" dialog. Answers immediately for the
    /// same reason [`Self::SoundPick`] does, and the result arrives the same
    /// way: as a `config_changed` event carrying the new `clip_dir`.
    ///
    /// The tray has had this dialog all along; this is the settings page
    /// asking for the same one, so both end at the same handler.
    FolderPick,
    /// Opens the "choose a sound" dialog. Answers immediately: the dialog
    /// outlives the request by as long as the user takes to browse, and the
    /// result arrives as a `config_changed` event, not as this reply.
    SoundPick,
    /// Plays the configured clip sound once, so the user can hear what they
    /// just chose without saving a clip.
    SoundTest,
    /// Ends the daemon process. Answered before the process exits, so the
    /// caller can tell a clean shutdown from a crashed socket.
    Shutdown,
}

impl Command {
    /// `Err` is the text of the error response — it is shown to whoever is
    /// driving the socket, so it names the command and the missing argument.
    pub fn parse(req: &Request) -> Result<Self, String> {
        let cmd = req.cmd.as_str();
        Ok(match cmd {
            "status" => Self::Status,
            "arm" => Self::Arm,
            "disarm" => Self::Disarm,
            "clip" => Self::Clip,
            "config.get" => Self::ConfigGet,
            "config.set" => {
                let values = match req.args.get("values") {
                    Some(Value::Object(map)) => map.clone(),
                    Some(_) => return Err("config.set: \"values\" must be an object".into()),
                    // Bare form: every argument other than id/cmd is a config key.
                    None => req.args.clone(),
                };
                if values.is_empty() {
                    return Err("config.set requires at least one key to set".into());
                }
                Self::ConfigSet(values)
            }
            "library.list" => Self::LibraryList {
                offset: usize_arg(req, "offset", 0)?,
                limit: usize_arg(req, "limit", DEFAULT_LIST_LIMIT)?.min(MAX_LIST_LIMIT),
            },
            "library.delete" => Self::LibraryDelete { clip_id: str_arg(req, "clip_id")? },
            "library.rename" => Self::LibraryRename {
                clip_id: str_arg(req, "clip_id")?,
                title: str_arg(req, "title")?,
            },
            "library.favorite" => Self::LibraryFavorite {
                clip_id: str_arg(req, "clip_id")?,
                favorite: bool_arg(req, "favorite")?,
            },
            "library.reveal" => Self::LibraryReveal { clip_id: str_arg(req, "clip_id")? },
            "library.copy" => Self::LibraryCopy { clip_id: str_arg(req, "clip_id")? },
            "library.keyframes" => Self::LibraryKeyframes { clip_id: str_arg(req, "clip_id")? },
            "library.frames" => Self::LibraryFrames { clip_id: str_arg(req, "clip_id")? },
            "library.export.start" => {
                let mut sync = req.clone();
                sync.cmd = "library.export".into();
                let Self::LibraryExport { clip_id, start_ms, end_ms, mode, output, tracks, size_limit_mb, vertical_center } = Self::parse(&sync)? else { unreachable!() };
                Self::LibraryExportStart { clip_id, start_ms, end_ms, mode, output, tracks, size_limit_mb, vertical_center, request_token: request_token_arg(req)? }
            },
            "library.export.status" => {
                let job_id = match req.args.get("job_id") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
                    _ => return Err("library.export.status requires a non-empty job_id string".into()),
                };
                let request_token = request_token_arg(req)?;
                if job_id.is_some() && request_token.is_some() { return Err("library.export.status accepts either job_id or request_token".into()); }
                Self::LibraryExportStatus { job_id, request_token }
            },
            "library.export.cancel" => {
                let job_id = str_arg(req, "job_id")?;
                if job_id.is_empty() { return Err("library.export.cancel requires a non-empty job_id".into()); }
                Self::LibraryExportCancel { job_id }
            },
            "library.export" => Self::LibraryExport {
                clip_id: str_arg(req, "clip_id")?,
                start_ms: u64_arg(req, "start_ms")?,
                end_ms: u64_arg(req, "end_ms")?,
                mode: req.args.get("mode").and_then(Value::as_str).unwrap_or("fast").to_string(),
                output: match req.args.get("output") {
                    None => ExportOutput::Master,
                    Some(Value::String(value)) if value == "master" => ExportOutput::Master,
                    Some(Value::String(value)) if value == "share" => ExportOutput::Share,
                    _ => return Err("library.export requires \"output\" to be \"master\" or \"share\"".into()),
                },
                tracks: tracks_arg(req)?,
                size_limit_mb: size_limit_arg(req)?,
                vertical_center: vertical_center_arg(req)?,
            },
            "library.tracks" => Self::LibraryTracks { clip_id: str_arg(req, "clip_id")? },
            "games.list" => Self::GamesList,
            "games.set" => Self::GamesSet {
                exe: str_arg(req, "exe")?,
                enabled: match req.args.get("enabled") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_bool().ok_or("games.set requires \"enabled\" to be a boolean")?),
                },
                name: match req.args.get("name") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_str().ok_or("games.set requires \"name\" to be a string")?.to_owned(),
                    ),
                },
                profile: req.args.get("profile").filter(|v| !v.is_null()).cloned(),
            },
            "games.remove" => Self::GamesRemove { exe: str_arg(req, "exe")? },
            "windows.list" => Self::WindowsList,
            "screenshot" => Self::Screenshot,
            "shots.list" => Self::ShotsList {
                offset: usize_arg(req, "offset", 0)?,
                limit: usize_arg(req, "limit", DEFAULT_LIST_LIMIT)?.min(MAX_LIST_LIMIT),
            },
            "shots.delete" => Self::ShotsDelete { shot_id: str_arg(req, "shot_id")? },
            "shots.reveal" => Self::ShotsReveal { shot_id: str_arg(req, "shot_id")? },
            "shots.copy" => Self::ShotsCopy { shot_id: str_arg(req, "shot_id")? },
            "monitors.list" => Self::MonitorsList,
            "encoders.list" => Self::EncodersList,
            "audio.apps" => Self::AudioApps,
            "stats.subscribe" => Self::StatsSubscribe { enabled: bool_arg(req, "enabled")? },
            "folder.pick" => Self::FolderPick,
            "sound.pick" => Self::SoundPick,
            "sound.test" => Self::SoundTest,
            "shutdown" => Self::Shutdown,
            other => return Err(format!("unknown command {other:?}")),
        })
    }
}

/// `tracks: [{index, gain}]`, absent or null meaning "keep the clip's audio
/// as it is". A listed entry with a bad shape is an error: dropping it would
/// silently export a clip without a track the user kept.
fn request_token_arg(req: &Request) -> Result<Option<String>, String> {
    match req.args.get("request_token") {
        None => Ok(None),
        Some(Value::String(token)) if !token.trim().is_empty() && token.len() <= 128 => Ok(Some(token.clone())),
        _ => Err(format!("{} requires request_token to be a non-empty string of at most 128 bytes", req.cmd)),
    }
}

/// `vertical_center`: absent or null keeps the source framing; otherwise the
/// centre of a 9:16 crop in thousandths of the width, 0..=1000.
fn vertical_center_arg(req: &Request) -> Result<Option<u16>, String> {
    match req.args.get("vertical_center") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(c @ 0..=1000) => Ok(Some(c as u16)),
            _ => Err(format!("{} requires \"vertical_center\" to be a whole number from 0 to 1000", req.cmd)),
        },
    }
}

/// `size_limit_mb`: absent or null for no limit; otherwise 1..=4000 MB, and
/// only meaningful for a precise share (the daemon refuses other pairings).
fn size_limit_arg(req: &Request) -> Result<Option<u32>, String> {
    match req.args.get("size_limit_mb") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(mb @ 1..=4000) => Ok(Some(mb as u32)),
            _ => Err(format!("{} requires \"size_limit_mb\" to be a whole number of megabytes from 1 to 4000", req.cmd)),
        },
    }
}

fn tracks_arg(req: &Request) -> Result<Option<Vec<ExportTrack>>, String> {
    let Some(value) = req.args.get("tracks").filter(|v| !v.is_null()) else { return Ok(None) };
    let bad = || format!("{} requires \"tracks\" to be a list of {{index, gain}}", req.cmd);
    let list = value.as_array().ok_or_else(bad)?;
    list.iter()
        .map(|entry| {
            let index = entry.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
            let gain = match entry.get("gain") {
                None => 100,
                Some(g) => g.as_u64().ok_or_else(bad)?,
            };
            Ok(ExportTrack {
                index: usize::try_from(index).map_err(|_| bad())?,
                gain: u32::try_from(gain.min(200)).unwrap_or(200),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

fn str_arg(req: &Request, key: &str) -> Result<String, String> {
    req.args
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{} requires a string {key:?}", req.cmd))
}

fn bool_arg(req: &Request, key: &str) -> Result<bool, String> {
    req.args
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{} requires a boolean {key:?}", req.cmd))
}

fn u64_arg(req: &Request, key: &str) -> Result<u64, String> {
    req.args
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{} requires a non-negative integer {key:?}", req.cmd))
}

/// An absent argument means `default`; a present one that is not a
/// non-negative whole number is an error, matching [`str_arg`] and
/// [`bool_arg`]. Silently defaulting a bad page number would let a UI's paging
/// bug read as an empty library instead of a mistake.
///
/// `try_from` rather than `as`: on a 32-bit target `as` would wrap a huge
/// value into a small one instead of rejecting it.
fn usize_arg(req: &Request, key: &str, default: usize) -> Result<usize, String> {
    match req.args.get(key) {
        None => Ok(default),
        Some(value) => value.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| {
            format!("{} requires {key:?} to be a non-negative whole number", req.cmd)
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::message::decode_request;

    fn parse(line: &str) -> (u64, Result<Command, String>) {
        let req = decode_request(line).expect("line should decode as a request");
        (req.id, Command::parse(&req))
    }

    #[test]
    fn async_request_tokens_are_bounded_and_status_selectors_exclusive() {
        for cmd in ["library.export.start", "library.export.status"] {
            for token in [json!(""),json!("  "),json!("x".repeat(129)),json!(null),json!(12)] {
                assert!(Command::parse(&req(cmd,json!({"clip_id":"x","start_ms":0,"end_ms":100,"request_token":token}))).is_err());
            }
        }
        assert!(Command::parse(&req("library.export.status",json!({"job_id":"job","request_token":"token"}))).is_err());
        assert_eq!(Command::parse(&req("library.export.status",json!({"request_token":"token"}))).unwrap(),Command::LibraryExportStatus {job_id:None,request_token:Some("token".into())});
        let parsed=Command::parse(&req("library.export.start",json!({"clip_id":"x","start_ms":0,"end_ms":100,"request_token":"x".repeat(128)}))).unwrap();
        assert!(matches!(parsed,Command::LibraryExportStart {request_token:Some(token),..} if token.len()==128));
    }
    #[test]
    fn async_export_commands_validate_and_preserve_start_arguments() {
        let args = json!({"clip_id":"2026-01-01_12-00-00", "start_ms":0, "end_ms":1000, "output":"share", "tracks":[{"index":1,"gain":80}]});
        let Command::LibraryExportStart { output, tracks, .. } = Command::parse(&req("library.export.start", args)).unwrap() else { panic!("start variant") };
        assert_eq!(output, ExportOutput::Share);
        assert_eq!(tracks.unwrap(), vec![ExportTrack {index:1,gain:80}]);
        assert_eq!(Command::parse(&req("library.export.status", json!({}))).unwrap(), Command::LibraryExportStatus {job_id:None,request_token:None});
        for cmd in ["library.export.status", "library.export.cancel"] {
            for value in [json!(""), json!(5), json!(false)] {
                assert!(Command::parse(&req(cmd, json!({"job_id":value}))).is_err());
            }
        }
        assert!(Command::parse(&req("library.export.cancel", json!({}))).is_err());
        assert!(Command::parse(&req("library.export.start", json!({"clip_id":"x"}))).is_err());
    }

    #[test]
    fn parses_the_spec_example() {
        let (id, cmd) = parse(r#"{"id":7,"cmd":"arm"}"#);
        assert_eq!(id, 7);
        assert_eq!(cmd.unwrap(), Command::Arm);
    }

    /// The whole reason parsing is two-stage: an unknown command must still
    /// leave the id available for the error response.
    #[test]
    fn unknown_command_fails_but_keeps_the_id() {
        let (id, cmd) = parse(r#"{"id":42,"cmd":"launch_missiles"}"#);
        assert_eq!(id, 42);
        assert!(cmd.unwrap_err().contains("launch_missiles"));
    }

    #[test]
    fn library_list_defaults_and_clamps() {
        let (_, cmd) = parse(r#"{"id":1,"cmd":"library.list"}"#);
        assert_eq!(cmd.unwrap(), Command::LibraryList { offset: 0, limit: DEFAULT_LIST_LIMIT });

        let (_, cmd) = parse(r#"{"id":1,"cmd":"library.list","offset":40,"limit":99999}"#);
        assert_eq!(
            cmd.unwrap(),
            Command::LibraryList { offset: 40, limit: MAX_LIST_LIMIT },
            "an unbounded limit would let one request materialize the entire library"
        );
    }

    /// A wrong-type page argument must be refused, not quietly replaced by the
    /// default — otherwise a UI's paging bug looks like an empty library
    /// rather than a mistake it can see and fix.
    #[test]
    fn a_wrong_type_page_argument_is_an_error_not_a_silent_default() {
        for line in [
            r#"{"id":1,"cmd":"library.list","offset":-5}"#,
            r#"{"id":1,"cmd":"library.list","offset":1.5}"#,
            r#"{"id":1,"cmd":"library.list","limit":"ten"}"#,
            r#"{"id":1,"cmd":"library.list","limit":null}"#,
        ] {
            let (_, cmd) = parse(line);
            let err = cmd.expect_err(&format!("{line} should be refused"));
            assert!(err.contains("library.list"), "error should name the command: {err}");
        }

        let (_, cmd) = parse(r#"{"id":1,"cmd":"library.list","offset":-5}"#);
        assert!(
            cmd.unwrap_err().contains("offset"),
            "the error should name which argument was wrong"
        );
    }

    #[test]
    fn missing_arguments_are_named_in_the_error() {
        let (_, cmd) = parse(r#"{"id":1,"cmd":"library.rename","id_":"x"}"#);
        let err = cmd.unwrap_err();
        assert!(err.contains("library.rename"), "error should name the command: {err}");
        assert!(err.contains("clip_id"), "error should name the missing argument: {err}");
    }

    #[test]
    fn the_dialog_commands_parse() {
        assert_eq!(parse(r#"{"id":0,"cmd":"folder.pick"}"#).1, Ok(Command::FolderPick));
        assert_eq!(parse(r#"{"id":1,"cmd":"sound.pick"}"#).1, Ok(Command::SoundPick));
        assert_eq!(parse(r#"{"id":2,"cmd":"sound.test"}"#).1, Ok(Command::SoundTest));
    }

    #[test]
    fn stats_subscribe_carries_its_flag() {
        let (_, cmd) = parse(r#"{"id":1,"cmd":"stats.subscribe","enabled":true}"#);
        assert_eq!(cmd.unwrap(), Command::StatsSubscribe { enabled: true });
    }

    /// A request with no id cannot be answered in the normal shape at all.
    #[test]
    fn a_line_without_an_id_is_not_a_request() {
        assert!(decode_request(r#"{"cmd":"arm"}"#).is_err());
        assert!(decode_request("not json at all").is_err());
    }

    /// The app sends this before replacing clippy-daemon.exe on disk. It is the
    /// only command whose success the caller confirms by watching the process
    /// leave, so it must parse from a bare request with no arguments.
    #[test]
    fn shutdown_parses_with_no_arguments() {
        let (id, cmd) = parse(r#"{"id":3,"cmd":"shutdown"}"#);
        assert_eq!(id, 3);
        assert_eq!(cmd.unwrap(), Command::Shutdown);
    }

    #[test]
    fn library_keyframes_needs_a_clip_id() {
        let req = Request {
            id: 1,
            cmd: "library.keyframes".into(),
            args: serde_json::from_str(r#"{"clip_id":"20260822_101500"}"#).unwrap(),
        };
        assert_eq!(
            Command::parse(&req),
            Ok(Command::LibraryKeyframes { clip_id: "20260822_101500".into() })
        );

        let bare = Request { id: 1, cmd: "library.keyframes".into(), args: Map::new() };
        assert!(Command::parse(&bare).is_err(), "a missing clip_id must be an error response");
    }

    #[test]
    fn library_frames_needs_a_clip_id() {
        assert_eq!(
            Command::parse(&req("library.frames", json!({"clip_id":"20260822_101500"}))),
            Ok(Command::LibraryFrames { clip_id: "20260822_101500".into() })
        );
        for args in [json!({}), json!({"clip_id":42}), json!({"clip_id":null})] {
            assert!(Command::parse(&req("library.frames", args)).is_err());
        }
    }

    #[test]
    fn library_export_defaults_mode_to_fast() {
        let req = Request {
            id: 2,
            cmd: "library.export".into(),
            args: serde_json::from_str(
                r#"{"clip_id":"20260822_101500","start_ms":1000,"end_ms":4000}"#,
            )
            .unwrap(),
        };
        assert_eq!(
            Command::parse(&req),
            Ok(Command::LibraryExport {
                clip_id: "20260822_101500".into(),
                start_ms: 1000,
                end_ms: 4000,
                mode: "fast".into(),
                output: ExportOutput::Master,
                tracks: None,
                size_limit_mb: None,
                vertical_center: None,
            }),
            "an omitted mode remains fast for existing clients"
        );
    }

    #[test]
    fn vertical_center_is_optional_and_bounded() {
        let base = json!({"clip_id":"20260822_101500","start_ms":0,"end_ms":1000,"mode":"precise","output":"share"});
        let with = |v: Value| { let mut a = base.clone(); a["vertical_center"] = v; a };
        assert!(matches!(Command::parse(&req("library.export", base.clone())).unwrap(),
            Command::LibraryExport { vertical_center: None, .. }));
        assert!(matches!(Command::parse(&req("library.export.start", with(json!(500)))).unwrap(),
            Command::LibraryExportStart { vertical_center: Some(500), .. }));
        assert!(matches!(Command::parse(&req("library.export", with(json!(0)))).unwrap(),
            Command::LibraryExport { vertical_center: Some(0), .. }));
        for bad in [json!(1001), json!(-1), json!(0.5), json!("500")] {
            assert!(Command::parse(&req("library.export", with(bad))).unwrap_err().contains("vertical_center"));
        }
    }

    #[test]
    fn size_limit_is_optional_bounded_and_carried_by_the_job_variant() {
        let base = json!({"clip_id":"20260822_101500","start_ms":0,"end_ms":1000,"mode":"precise","output":"share"});
        let with = |v: Value| { let mut a = base.clone(); a["size_limit_mb"] = v; a };
        assert!(matches!(Command::parse(&req("library.export", base.clone())).unwrap(),
            Command::LibraryExport { size_limit_mb: None, .. }));
        assert!(matches!(Command::parse(&req("library.export.start", with(json!(20)))).unwrap(),
            Command::LibraryExportStart { size_limit_mb: Some(20), .. }));
        assert!(matches!(Command::parse(&req("library.export", with(json!(null)))).unwrap(),
            Command::LibraryExport { size_limit_mb: None, .. }));
        for bad in [json!(0), json!(4001), json!(-5), json!(2.5), json!("20")] {
            assert!(Command::parse(&req("library.export", with(bad))).unwrap_err().contains("size_limit_mb"));
        }
    }

    #[test]
    fn library_copy_names_one_clip() {
        assert_eq!(
            Command::parse(&req("library.copy", json!({"clip_id":"20260822_101500"}))),
            Ok(Command::LibraryCopy { clip_id: "20260822_101500".into() })
        );
        assert!(Command::parse(&req("library.copy", json!({}))).is_err());
    }

    #[test]
    fn library_export_output_is_independent_of_precision_and_validated() {
        for (wire, expected) in [("master", ExportOutput::Master), ("share", ExportOutput::Share)] {
            let parsed = Command::parse(&req("library.export", json!({
                "clip_id":"20260822_101500", "start_ms":0, "end_ms":1000,
                "mode":"precise", "output":wire
            }))).unwrap();
            assert!(matches!(parsed, Command::LibraryExport { output, mode, .. }
                if output == expected && mode == "precise"));
        }
        for output in [json!("shrae"), json!(null), json!(true), json!(42)] {
            let error = Command::parse(&req("library.export", json!({
                "clip_id":"20260822_101500", "start_ms":0, "end_ms":1000,
                "output":output
            }))).unwrap_err();
            assert!(error.contains("output"));
        }
    }

    #[test]
    fn library_export_requires_both_bounds() {
        let req = Request {
            id: 3,
            cmd: "library.export".into(),
            args: serde_json::from_str(r#"{"clip_id":"20260822_101500","start_ms":1000}"#).unwrap(),
        };
        let error = Command::parse(&req).expect_err("a missing end_ms must be refused");
        assert!(error.contains("end_ms"), "the message must name the missing argument: {error}");
    }

    fn req(cmd: &str, args: serde_json::Value) -> Request {
        Request { id: 1, cmd: cmd.to_string(), args: args.as_object().cloned().unwrap_or_default() }
    }

    #[test]
    fn the_shots_commands_parse_with_their_arguments() {
        assert_eq!(Command::parse(&req("screenshot", json!({}))).unwrap(), Command::Screenshot);
        assert_eq!(
            Command::parse(&req("shots.list", json!({}))).unwrap(),
            Command::ShotsList { offset: 0, limit: DEFAULT_LIST_LIMIT },
            "shots.list must default exactly as library.list does"
        );
        assert_eq!(
            Command::parse(&req("shots.list", json!({"offset": 5, "limit": 9000}))).unwrap(),
            Command::ShotsList { offset: 5, limit: MAX_LIST_LIMIT },
            "the page ceiling is shared with library.list, not reinvented"
        );
        for (cmd, expected) in [
            ("shots.delete", Command::ShotsDelete { shot_id: "20260827_143012".into() }),
            ("shots.reveal", Command::ShotsReveal { shot_id: "20260827_143012".into() }),
            ("shots.copy", Command::ShotsCopy { shot_id: "20260827_143012".into() }),
        ] {
            assert_eq!(
                Command::parse(&req(cmd, json!({"shot_id": "20260827_143012"}))).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn a_shots_command_without_its_id_is_refused_by_name() {
        for cmd in ["shots.delete", "shots.reveal", "shots.copy"] {
            let err = Command::parse(&req(cmd, json!({}))).expect_err("must not parse");
            // The message reaches whoever is driving the socket, so it names both
            // the command and the argument it wanted.
            assert!(err.contains(cmd), "{err:?} should name the command");
            assert!(err.contains("shot_id"), "{err:?} should name the argument");
        }
    }

    #[test]
    fn export_tracks_and_game_commands_parse() {
        let parsed = Command::parse(&req(
            "library.export",
            json!({"clip_id":"20260822_101500","start_ms":0,"end_ms":1000,
                   "tracks":[{"index":1,"gain":50},{"index":2},{"index":3,"gain":900}]}),
        ))
        .unwrap();
        let Command::LibraryExport { tracks: Some(tracks), .. } = parsed else { panic!("tracks") };
        assert_eq!(
            tracks,
            [
                ExportTrack { index: 1, gain: 50 },
                ExportTrack { index: 2, gain: 100 },
                ExportTrack { index: 3, gain: 200 },
            ]
        );
        assert!(
            Command::parse(&req(
                "library.export",
                json!({"clip_id":"20260822_101500","start_ms":0,"end_ms":1,"tracks":[{"gain":5}]})
            ))
            .is_err()
        );
        assert_eq!(Command::parse(&req("games.list", json!({}))).unwrap(), Command::GamesList);
        assert_eq!(Command::parse(&req("windows.list", json!({}))).unwrap(), Command::WindowsList);
        assert_eq!(
            Command::parse(&req("games.set", json!({"exe":"a.exe","enabled":false}))).unwrap(),
            Command::GamesSet { exe: "a.exe".into(), enabled: Some(false), name: None, profile: None }
        );
        assert!(Command::parse(&req("games.set", json!({"exe":"a.exe","enabled":"no"}))).is_err());
        assert!(Command::parse(&req("games.remove", json!({}))).is_err());
        assert_eq!(
            Command::parse(&req("library.tracks", json!({"clip_id":"x"}))).unwrap(),
            Command::LibraryTracks { clip_id: "x".into() }
        );
    }
}
