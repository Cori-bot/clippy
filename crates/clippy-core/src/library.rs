//! The clip library's on-disk format.
//!
//! One `.mp4` and one `.json` sidecar sharing a stem, in one directory; the
//! directories are the clips folder and one sub-folder per game (see the
//! per-game folder helpers below).
//! Not a database: a crash mid-write costs one clip's metadata instead of the
//! library, deleting an `.mp4` in Explorer leaves an orphan that the next scan
//! prunes rather than a phantom row, and SQLite would be a C dependency and a
//! ~1 MB binary bump against a 1.5 MB engine. See spec §5.2.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clippy_proto::ClipMeta;
use windows::Win32::System::{
    SystemInformation::GetLocalTime,
    SystemServices::TIME_ZONE_ID_DAYLIGHT,
    Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION},
};

pub fn mp4_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.mp4"))
}

pub fn sidecar_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Thumbnails are written by the daemon starting in stage 3; the path shape
/// is fixed here so deletion already removes them.
pub fn thumb_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.jpg"))
}

/// True if `id` is a syntactically valid clip id: `YYYYMMDD_HHMMSS`, with an
/// optional `_N` collision suffix.
///
/// Ids arrive from the control socket and are concatenated into filesystem
/// paths. Everything that turns an id into a path calls this first — a
/// whitelist of digits and two underscores cannot express `..`, a separator,
/// a drive letter, or a UNC prefix.
pub fn is_valid_id(id: &str) -> bool {
    let Some((date, rest)) = id.split_once('_') else { return false };
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if date.len() != 8 || !all_digits(date) {
        return false;
    }
    match rest.split_once('_') {
        None => rest.len() == 6 && all_digits(rest),
        Some((time, suffix)) => time.len() == 6 && all_digits(time) && all_digits(suffix),
    }
}

/// Proves `dir` can actually take clips: creates it if missing, then writes a
/// probe file and removes it.
///
/// The probe write is the whole point, and the reason this is not simply
/// `create_dir_all`. That call returns `Ok` for a directory that already exists
/// and cannot be written to — `C:\`, `C:\Program Files`, a read-only network
/// share — so a create-only check would accept a folder and leave the user to
/// discover the truth when a clip fails to save, which is the one moment they
/// least want to read an error. Roughly a millisecond, and it answers the
/// question that was actually asked.
///
/// Called when the clip directory *changes* and before an `arm`, never per
/// clip: [`allocate_clip_id`] below stays a bare `create_dir_all` and needs no
/// probe of its own. It never did — the save path was already writing an MP4,
/// which tells it everything a probe would — and now even less so, since the
/// reservation it takes is itself a real create that fails the same way.
pub fn ensure_writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("impossible de créer le dossier des clips {}", dir.display()))?;
    // Named per process so a daemon and a `clippy replay` pointed at one folder
    // cannot race on a single name and delete each other's probe.
    let probe = dir.join(format!(".clippy-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"clippy").with_context(|| {
        format!("le dossier des clips {} n’est pas accessible en écriture", dir.display())
    })?;
    // Removal is cleanup, not the check. A probe left behind is untidy — it
    // would sit in Explorer beside the clips — but the write already proved
    // what the caller asked, so failing to remove it must not fail the call.
    if let Err(error) = std::fs::remove_file(&probe) {
        tracing::warn!(path = %probe.display(), %error, "could not remove the clip directory probe");
    }
    Ok(())
}

/// Picks an unused id for a clip being saved now, creating `dir` if needed,
/// and **reserves it by creating the `.mp4` empty**.
///
/// The reservation is not a detail — it is the contract. Ids are wall-clock
/// stamps (`YYYYMMDD_HHMMSS`), so two savers in the same second want the same
/// name, and this used to answer by *asking* whether the file existed and
/// creating nothing. That was safe only for as long as the single caller held
/// a lock across the gap between allocating and writing. `Daemon::export_clip`
/// broke that assumption: it takes no lock and spends a whole keyframe demux
/// of the source between the two, so a clip taken with the hotkey mid-export
/// could be handed the same id, write its footage there, and have the export's
/// sink writer truncate it moments later — captured footage destroyed.
///
/// So the name is claimed with `create_new`, which is atomic against every
/// other thread and every other process. Two costs come with it, and both are
/// the caller's to pay:
///
/// - **The caller owns the file from here on.** Every path out of a save must
///   either overwrite the reservation (both writers here go through
///   `MFCreateSinkWriterFromURL`, which creates *and truncates*, so a zero-byte
///   file at the path is exactly as good as no file) or delete it. See
///   `replay::save_clip` and `Daemon::export_clip`.
/// - **A reservation can be stranded** if the process dies between claiming and
///   writing — the build is `panic = "abort"`. [`scan`] therefore skips
///   zero-byte `.mp4`s, which are never playable clips anyway.
pub fn allocate_clip_id(dir: &Path) -> Result<String> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("impossible de créer le dossier des clips {}", dir.display()))?;
    next_free_id(dir, &now_id_stamp())
}

/// The `YYYYMMDD_HHMMSS` stamp every id starts from, in local time.
///
/// Extracted so screenshots start from the same stamp clips do. Two
/// hand-written `format!`s would be two chances for the id grammars to drift,
/// and `is_valid_id` — which guards every path built from an id — accepts only
/// one of them.
pub fn now_id_stamp() -> String {
    let now = unsafe { GetLocalTime() };
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    )
}

/// Two clips in the same second get `_2`, `_3`, … rather than one silently
/// overwriting the other, and the winner of that race is decided by the
/// filesystem rather than by a check-then-act.
///
/// `create_new` is the whole mechanism: it creates the file or fails with
/// `AlreadyExists`, in one syscall, so two callers cannot both come away
/// believing they own the same id. An `exists()` probe could — see
/// [`allocate_clip_id`] for the concrete way that lost captured footage.
/// The returned id names a real, zero-byte file that the caller now owns.
///
/// Any error other than `AlreadyExists` stops the loop rather than advancing
/// the suffix: a directory that cannot be written to would otherwise be
/// mistaken for one that is full, and the caller would wait out four billion
/// failing `open` calls to be told the wrong thing.
///
/// Returns `Result` rather than ending in an `unreachable!()`. The loop below
/// genuinely cannot run out — it would need `u32::MAX` clips written within one
/// second — but this function sits on a path a socket message reaches
/// (`clip` → `EngineCommand::Clip` → `save_clip` → `allocate_clip_id`), and the
/// build is `panic = "abort"`, where a panicking call anywhere on such a path is
/// forbidden outright rather than argued about. The caller already returns
/// `Result`, so honouring it costs a bounded range and one `bail!`, and removes
/// the argument entirely.
fn next_free_id(dir: &Path, stem: &str) -> Result<String> {
    if reserve(dir, stem)? {
        return Ok(stem.to_string());
    }
    for n in 2u32..=u32::MAX {
        let candidate = format!("{stem}_{n}");
        if reserve(dir, &candidate)? {
            return Ok(candidate);
        }
    }
    bail!(
        "aucun identifiant de clip libre pour {stem} — le dossier des clips contient déjà tous les suffixes"
    )
}

/// `Ok(true)` when this call created `dir/id.mp4` and now owns it, `Ok(false)`
/// when something already held that name, `Err` for anything else.
fn reserve(dir: &Path, id: &str) -> Result<bool> {
    let path = mp4_path(dir, id);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("impossible de réserver {}", path.display())),
    }
}

/// Writes the sidecar to a temp file and renames it into place, so a crash
/// mid-write cannot leave half-parsed JSON beside a good MP4.
pub fn write_sidecar(dir: &Path, meta: &ClipMeta) -> Result<()> {
    let final_path = sidecar_path(dir, &meta.id);
    let temp_path = final_path.with_extension("json.tmp");
    let json =
        serde_json::to_string_pretty(meta).context("sérialisation des métadonnées du clip")?;
    std::fs::write(&temp_path, json)
        .with_context(|| format!("écriture de {}", temp_path.display()))?;
    // Windows rename fails if the destination exists; rewriting a sidecar
    // (rename, favorite) is a normal operation.
    let _ = std::fs::remove_file(&final_path);
    std::fs::rename(&temp_path, &final_path)
        .with_context(|| format!("renommage vers {}", final_path.display()))
}

pub fn read_sidecar(path: &Path) -> Result<ClipMeta> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("lecture de {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("analyse de {}", path.display()))
}

/// Every clip in `dir`, newest first.
///
/// An `.mp4` with no sidecar gets one synthesized from the filesystem, so a
/// file dropped in by hand still shows up. A `.json` with no `.mp4` is an
/// orphan and is skipped. A malformed sidecar is warned about and its clip is
/// adopted as if the sidecar were missing — one bad file never hides a clip.
///
/// A **zero-byte** `.mp4` is skipped. It is never a playable clip, so there is
/// nothing to lose, and it closes two ways a broken grid card could otherwise
/// appear: an [`allocate_clip_id`] reservation stranded by a process that died
/// before writing it (the build is `panic = "abort"`), and an export whose
/// cleanup delete was refused — Defender holding a handle on the file it just
/// scanned is the everyday cause. Either would otherwise be adopted as a clip
/// with `duration_ms: 0` that will not play and cannot be trimmed.
pub fn scan(dir: &Path) -> Result<Vec<ClipMeta>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // No directory yet simply means no clips yet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };

    let mut clips = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mp4") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if !is_valid_id(id) {
            continue;
        }
        // A reservation or a failed export's leftovers, not a clip. Taken off
        // the directory entry rather than with a fresh `metadata` call — on
        // Windows the size came back with the enumeration, so this costs
        // nothing. A metadata error is treated as "not zero": the file is
        // there, something is wrong with reading it, and hiding a clip is the
        // worse of the two mistakes.
        if entry.metadata().is_ok_and(|m| m.len() == 0) {
            continue;
        }
        let sidecar = sidecar_path(dir, id);
        let meta = match read_sidecar(&sidecar) {
            Ok(meta) => meta,
            Err(e) if sidecar.exists() => {
                tracing::warn!(clip = id, error = %e, "unreadable sidecar, adopting the clip");
                adopt(dir, id)
            }
            // The sidecar is written after the MP4 is finalized, so a writer
            // killed mid-file (power cut, task manager, `panic = "abort"`)
            // leaves an MP4 with no sidecar and no `moov`: unplayable, and
            // adopting it shows a 0:00 card that cannot be opened or trimmed.
            Err(_) if unfinished_mp4(&path) => {
                tracing::debug!(clip = id, "skipping an unfinished mp4");
                continue;
            }
            Err(_) => adopt(dir, id),
        };
        clips.push(meta);
    }
    // Ids are zero-padded timestamps, so lexicographic order is chronological.
    clips.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(clips)
}

/// An MP4 that starts like one (`ftyp` first) but whose top-level boxes end
/// without a `moov`: what Media Foundation leaves when the writer dies before
/// `Finalize`. Anything that does not start with `ftyp` is not judged (a
/// hand-dropped file is adopted as before), and a read error counts as
/// finished: hiding a clip is the worse mistake.
fn unfinished_mp4(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let Ok(len) = file.metadata().map(|m| m.len()) else { return false };
    let mut at = 0u64;
    for index in 0..1024 {
        let mut head = [0u8; 8];
        if at + 8 > len || file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut head).is_err() {
            return index > 0;
        }
        let kind = &head[4..8];
        if index == 0 && kind != b"ftyp" {
            return false;
        }
        if kind == b"moov" {
            return false;
        }
        let size = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64 {
            1 => {
                let mut large = [0u8; 8];
                if file.read_exact(&mut large).is_err() {
                    return true;
                }
                u64::from_be_bytes(large)
            }
            size => size,
        };
        // 0 means "to the end of the file"; below 8 is not a box. Either way
        // nothing after it can be a `moov`.
        if size < 8 {
            return true;
        }
        at = at.saturating_add(size);
    }
    false
}

// ---------------------------------------------------------------------------
// Per-game folders
//
// Clips live in `<clips folder>\<folder>\<id>.mp4`, one sub-folder per game and
// one for the desktop, the way NVIDIA's overlay files them. The folder is part
// of a clip's identity (`ClipMeta::folder`), and the single-directory functions
// above keep working on whichever directory they are handed: the helpers here
// only decide *which* directories exist and walk them.
// ---------------------------------------------------------------------------

/// The sub-folder desktop clips go in. Screenshots keep their own folder.
pub const DESKTOP_FOLDER: &str = "Bureau";

/// A folder name no game may take: the screenshots' own folder.
/// Rebuildable editor audio, never a user-visible game folder.
pub const TRACK_CACHE_DIR: &str = ".clippy-cache";
const RESERVED_FOLDERS: &[&str] = &["screenshots", TRACK_CACHE_DIR];

/// Longest folder name used; Windows allows far more, but a game's name is not
/// worth a path that approaches MAX_PATH once an id is added.
const MAX_FOLDER_LEN: usize = 60;

/// Turns a game's display name into a single, safe path component, or `None`
/// when nothing usable is left (all punctuation, or a reserved name).
///
/// Windows' forbidden characters and control characters become nothing,
/// runs of whitespace collapse, and trailing dots and spaces -- which Windows
/// silently strips, making two names one folder -- are removed. The result
/// passes [`is_valid_folder`] by construction.
pub fn sanitize_folder(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String =
        collapsed.trim_matches(|c| c == '.' || c == ' ').chars().take(MAX_FOLDER_LEN).collect();
    let trimmed = trimmed.trim_end_matches(|c| c == '.' || c == ' ').to_string();
    is_valid_folder(&trimmed).then_some(trimmed)
}

/// Whether `folder` is one safe path component to put under the clips folder.
///
/// Like [`is_valid_id`], this guards a string that ends up in a path -- the
/// folder comes out of a sidecar or the game registry, both plain files a
/// person can edit -- so it is a whitelist-by-rejection of everything that can
/// escape: separators, drive colons, `..`, device names, reserved names.
pub fn is_valid_folder(folder: &str) -> bool {
    if folder.is_empty() || folder.chars().count() > MAX_FOLDER_LEN {
        return false;
    }
    if folder == "." || folder == ".." || folder.starts_with(' ') || folder.ends_with([' ', '.']) {
        return false;
    }
    if folder.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return false;
    }
    let lower = folder.to_lowercase();
    if RESERVED_FOLDERS.contains(&lower.as_str()) {
        return false;
    }
    // DOS device names are files, not folders, whatever the extension.
    let stem = lower.split('.').next().unwrap_or("");
    const DEVICES: &[&str] = &["con", "prn", "aux", "nul"];
    let numbered = |p: &str| {
        stem.strip_prefix(p)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
    };
    !(DEVICES.contains(&stem) || numbered("com") || numbered("lpt"))
}

/// The directory a folder name denotes under the clips folder; the clips folder
/// itself for the empty name. Returns `None` for a name that is not
/// [`is_valid_folder`], so a hostile sidecar cannot point outside it.
pub fn folder_dir(clip_dir: &Path, folder: &str) -> Option<PathBuf> {
    if folder.is_empty() {
        Some(clip_dir.to_path_buf())
    } else if is_valid_folder(folder) {
        Some(clip_dir.join(folder))
    } else {
        None
    }
}

/// The clips folder and each valid sub-folder directly under it, as
/// `(folder, directory)`; the clips folder itself comes first with `""`.
/// Does not descend further: clips are exactly one level deep.
pub fn folder_dirs(clip_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found = vec![(String::new(), clip_dir.to_path_buf())];
    let Ok(entries) = std::fs::read_dir(clip_dir) else { return found };
    let mut subs: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            is_valid_folder(&name).then(|| (name, e.path()))
        })
        .collect();
    subs.sort();
    found.extend(subs);
    found
}

/// Every clip under the clips folder: the folder itself and each sub-folder,
/// newest first, each carrying the folder it was found in (whatever its sidecar
/// says -- a clip moved by hand belongs where it is).
pub fn scan_all(clip_dir: &Path) -> Result<Vec<ClipMeta>> {
    let mut all = Vec::new();
    for (folder, dir) in folder_dirs(clip_dir) {
        // The clips folder itself must exist-or-not like `scan` says; a broken
        // sub-folder is skipped with a warning rather than hiding the others.
        match scan(&dir) {
            Ok(mut clips) => {
                for clip in &mut clips {
                    clip.folder = folder.clone();
                }
                all.append(&mut clips);
            }
            Err(e) if folder.is_empty() => return Err(e),
            Err(e) => {
                tracing::warn!(folder = %folder, error = %format!("{e:#}"), "skipping an unreadable clips sub-folder")
            }
        }
    }
    all.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(all)
}

/// Like [`allocate_clip_id`], but in `clip_dir\folder` and unique across every
/// folder: the library is keyed by bare id, so two games' clips made in the
/// same second must not share one.
pub fn allocate_clip_id_in(clip_dir: &Path, folder: &str) -> Result<String> {
    let dir = folder_dir(clip_dir, folder)
        .with_context(|| format!("nom de dossier de clips invalide : {folder:?}"))?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("impossible de créer le dossier des clips {}", dir.display()))?;
    let others: Vec<PathBuf> = folder_dirs(clip_dir).into_iter().map(|(_, d)| d).collect();
    let taken = |id: &str| others.iter().any(|d| mp4_path(d, id).exists());
    let stamp = now_id_stamp();
    let mut candidate = stamp.clone();
    for n in 2u32..=u32::MAX {
        if !taken(&candidate) && reserve(&dir, &candidate)? {
            return Ok(candidate);
        }
        candidate = format!("{stamp}_{n}");
    }
    bail!("aucun identifiant de clip libre pour {stamp}")
}

/// Removes only this clip's editor audio, from the private cache and the
/// legacy loose-file location. The master MP4 and neighboring clips survive.
pub fn remove_track_files(dir: &Path, id: &str) {
    if !is_valid_id(id) {
        return;
    }
    let cache = dir.join(TRACK_CACHE_DIR);
    for location in [dir, cache.as_path()] {
        let Ok(entries) = std::fs::read_dir(location) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(track) = name.strip_prefix(&format!("{id}.t")) else { continue };
            let Some(ordinal) =
                track.strip_suffix(".part.m4a").or_else(|| track.strip_suffix(".m4a"))
            else {
                continue;
            };
            if !ordinal.is_empty() && ordinal.bytes().all(|c| c.is_ascii_digit()) {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    tracing::warn!(path = %entry.path().display(), %error, "could not remove cached editor audio");
                }
            }
        }
    }
}

/// Best-effort metadata for an `.mp4` with no usable sidecar. The fields that
/// need a demuxer to recover are left at zero rather than guessed.
fn adopt(dir: &Path, id: &str) -> ClipMeta {
    let bytes = std::fs::metadata(mp4_path(dir, id)).map(|m| m.len()).unwrap_or(0);
    ClipMeta {
        id: id.to_string(),
        title: format!("clip_{id}"),
        created: created_from_id(id),
        duration_ms: 0,
        bytes,
        width: 0,
        height: 0,
        fps: 0,
        encoder: String::new(),
        has_audio: false,
        favorite: false,
        audio_tracks: Vec::new(),
        game: String::new(),
        folder: String::new(),
    }
}

/// `20260726_143012` -> `2026-07-26T14:30:12` with no offset claimed, since
/// the time zone in force when the file was made -- an adopted clip, or a
/// screenshot, whose id is stamped straight from this same format -- is not
/// recoverable from a filename alone.
pub fn created_from_id(id: &str) -> String {
    let d = &id[..8];
    let t = &id[9..15];
    format!("{}-{}-{}T{}:{}:{}", &d[..4], &d[4..6], &d[6..8], &t[..2], &t[2..4], &t[4..6])
}

/// Deletes the oldest non-favorite clips until the library fits under
/// `max_gb`, returning the ids removed, oldest first (spec §5.4).
///
/// `max_gb == 0` disables the ceiling and deletes nothing.
///
/// Favorites are never deleted, even if the favorites alone exceed the
/// ceiling — starring a clip is the user saying "keep this", and the ceiling
/// is a convenience, not a quota. When that happens the library is left over
/// budget and this logs it; the alternative is deleting the one clip the user
/// explicitly protected.
///
/// A clip that fails to delete is logged and skipped rather than aborting the
/// prune: this runs immediately after a clip save, and one locked file must
/// not stop the ceiling from doing its job for every other clip. The `.mp4` is
/// what decides — same rule as the daemon's `delete`. If it will not go, the
/// clip still holds its bytes and is not reported as removed; only once it is
/// gone are the sidecar and thumbnail cleaned up best-effort. Counting a clip
/// as freed while its `.mp4` is still on disk would have the ceiling stop
/// pruning against space it never actually reclaimed.
pub fn prune_to_ceiling(dir: &Path, max_gb: u32) -> Result<Vec<String>> {
    if max_gb == 0 {
        return Ok(Vec::new());
    }
    let ceiling = u64::from(max_gb).saturating_mul(1_000_000_000);

    // `scan_all` returns newest first; the ceiling deletes oldest first. `dir`
    // is the clips folder: the ceiling counts every game's folder together.
    let mut clips = scan_all(dir)?;
    clips.reverse();

    let mut held: u64 = clips.iter().map(|c| c.bytes).sum();
    let mut deleted = Vec::new();

    for clip in &clips {
        if held <= ceiling {
            break;
        }
        if clip.favorite {
            continue;
        }

        let Some(clip_folder) = folder_dir(dir, &clip.folder) else { continue };
        match std::fs::remove_file(mp4_path(&clip_folder, &clip.id)) {
            Ok(()) => {}
            // Already gone: it is not holding those bytes either, and its
            // companions are litter `scan` would never surface again.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(
                    clip = clip.id,
                    error = %e,
                    "ceiling could not delete a clip; keeping it and moving on"
                );
                continue;
            }
        }

        for path in [sidecar_path(&clip_folder, &clip.id), thumb_path(&clip_folder, &clip.id)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "the ceiling deleted a clip but not one of its companion files"
                ),
            }
        }

        remove_track_files(&clip_folder, &clip.id);
        held = held.saturating_sub(clip.bytes);
        deleted.push(clip.id.clone());
    }

    if held > ceiling {
        tracing::warn!(
            held_bytes = held,
            ceiling_bytes = ceiling,
            "the clip library is over its ceiling and nothing left is safe to delete"
        );
    }
    Ok(deleted)
}

/// Local-time RFC 3339 stamp for a clip being saved now.
pub fn now_rfc3339_local() -> String {
    let now = unsafe { GetLocalTime() };
    let mut tz = TIME_ZONE_INFORMATION::default();
    // Windows defines UTC = local + Bias, so the ISO offset is the negation,
    // and the active seasonal bias has to be folded in or half the year is
    // reported an hour out.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias =
        tz.Bias + if id == TIME_ZONE_ID_DAYLIGHT { tz.DaylightBias } else { tz.StandardBias };
    format_rfc3339(now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, -bias)
}

/// Split out from [`now_rfc3339_local`] so the offset arithmetic is testable
/// without running in a particular time zone.
pub fn format_rfc3339(
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    offset_minutes: i32,
) -> String {
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let abs = offset_minutes.unsigned_abs();
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{sign}{:02}:{:02}",
        abs / 60,
        abs % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Istanbul is UTC+3; Windows reports that as a Bias of -180 minutes.
    #[test]
    fn rfc3339_renders_a_positive_offset() {
        assert_eq!(format_rfc3339(2026, 7, 26, 14, 30, 12, 180), "2026-07-26T14:30:12+03:00");
    }

    #[test]
    fn rfc3339_renders_a_negative_and_a_zero_offset() {
        assert_eq!(format_rfc3339(2026, 1, 2, 3, 4, 5, -300), "2026-01-02T03:04:05-05:00");
        assert_eq!(format_rfc3339(2026, 1, 2, 3, 4, 5, 0), "2026-01-02T03:04:05+00:00");
        assert_eq!(
            format_rfc3339(2026, 1, 2, 3, 4, 5, 330),
            "2026-01-02T03:04:05+05:30",
            "half-hour zones are real (India, Newfoundland) and must not truncate"
        );
    }

    /// Clip ids arrive over the control socket and get concatenated into
    /// paths. This is the check that keeps them inside the clip directory.
    #[test]
    fn id_validation_rejects_anything_that_could_escape_the_clip_dir() {
        assert!(is_valid_id("20260726_143012"));
        assert!(is_valid_id("20260726_143012_2"));

        for bad in [
            "",
            "..",
            "../../Windows/System32/config/SAM",
            r"..\..\secrets",
            "20260726_143012/../x",
            r"C:\Windows\System32",
            r"\\server\share\x",
            "20260726",
            "2026072_143012",
            "20260726_14301",
            "20260726_143012_",
            "20260726_143012_a",
            "20260726_1430l2",
            "20260726_143012.mp4",
        ] {
            assert!(!is_valid_id(bad), "{bad:?} must be rejected");
        }
    }

    /// The check behind "change clips folder" and the pre-arm preflight.
    ///
    /// Creating the directory is the easy half. The half that matters is the
    /// probe write: `create_dir_all` returns `Ok` for a directory that already
    /// exists and cannot be written to, so a create-only check would accept
    /// `C:\`, `C:\Program Files`, or a read-only share and hand the user back
    /// exactly the failure this function exists to prevent — one discovered at
    /// clip time.
    #[test]
    fn ensure_writable_creates_the_directory_and_leaves_nothing_behind() {
        let root = std::env::temp_dir().join(format!("clippy-ensure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // Missing, and nested: the whole chain gets created.
        let nested = root.join("a").join("b");
        ensure_writable(&nested).expect("a fresh nested path must be created");
        assert!(nested.is_dir(), "the directory must exist afterwards");

        // The probe must not survive. A stray file in the clip directory would
        // show up in Explorer and in `scan`.
        let left: Vec<_> = std::fs::read_dir(&nested).unwrap().map(|e| e.unwrap().path()).collect();
        assert!(left.is_empty(), "the probe file must be cleaned up: {left:?}");

        // Idempotent: an existing writable directory passes.
        ensure_writable(&nested).expect("an existing writable directory must pass");

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A path that cannot be a directory must be refused rather than reported
    /// usable. Pointing the parent at a *file* forces the failure portably —
    /// no admin rights, no unplugged drive, no network share needed.
    #[test]
    fn ensure_writable_refuses_a_path_that_cannot_be_a_directory() {
        let root = std::env::temp_dir().join(format!("clippy-ensure-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let file = root.join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();

        // The clip directory itself is a file.
        assert!(ensure_writable(&file).is_err(), "an existing file is not a usable clip directory");
        // The clip directory's parent is a file.
        assert!(
            ensure_writable(&file.join("child")).is_err(),
            "a directory under a file cannot be created"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_do_not_collide_within_one_second() {
        let dir = std::env::temp_dir().join(format!("clippy-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("20260726_143012.mp4"), b"x").unwrap();
        let next = next_free_id(&dir, "20260726_143012").unwrap();
        assert_eq!(next, "20260726_143012_2");

        std::fs::write(dir.join("20260726_143012_2.mp4"), b"x").unwrap();
        assert_eq!(next_free_id(&dir, "20260726_143012").unwrap(), "20260726_143012_3");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The reservation race, which an `exists()` probe could not close: the
    /// two callers of `allocate_clip_id` no longer serialize (`export_clip`
    /// takes no lock and demuxes the whole source between allocating and
    /// writing), so two allocations that write nothing in between must still
    /// come back with different ids. Before `create_new` this returned
    /// `_2` twice and the second writer destroyed the first one's footage.
    #[test]
    fn an_allocated_id_is_reserved_even_before_anything_is_written() {
        let dir = std::env::temp_dir().join(format!("clippy-reserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let first = next_free_id(&dir, "20260726_143012").unwrap();
        let second = next_free_id(&dir, "20260726_143012").unwrap();
        let third = next_free_id(&dir, "20260726_143012").unwrap();
        assert_eq!(
            [first.as_str(), second.as_str(), third.as_str()],
            ["20260726_143012", "20260726_143012_2", "20260726_143012_3",]
        );

        // The reservation is a real, empty file: real so no other process can
        // take the name, empty so the writer that follows can simply truncate
        // it (both writers go through `MFCreateSinkWriterFromURL`, which
        // creates *and* truncates).
        for id in [&first, &second, &third] {
            let path = mp4_path(&dir, id);
            assert!(path.is_file(), "{id} must be reserved on disk");
            assert_eq!(std::fs::metadata(&path).unwrap().len(), 0, "{id} must be empty");
        }

        // And a reservation is not a clip: nothing that was never written
        // reaches the grid.
        assert!(scan(&dir).unwrap().is_empty(), "zero-byte reservations are not clips");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_skips_orphaned_sidecars_and_adopts_bare_mp4s() {
        let dir = std::env::temp_dir().join(format!("clippy-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A complete clip.
        std::fs::write(dir.join("20260726_100000.mp4"), b"video").unwrap();
        write_sidecar(&dir, &sample_meta("20260726_100000")).unwrap();
        // A sidecar whose mp4 was deleted in Explorer: pruned, not listed.
        write_sidecar(&dir, &sample_meta("20260726_090000")).unwrap();
        // An mp4 with no sidecar: adopted so hand-dropped files still appear.
        std::fs::write(dir.join("20260726_110000.mp4"), b"video").unwrap();
        // A zero-byte mp4: a stranded `allocate_clip_id` reservation, or an
        // export whose cleanup delete was refused. Never playable, so never
        // listed -- even though it has a perfectly valid sidecar beside it,
        // which is what a `save_clip` killed between the write and the mux
        // would look like.
        std::fs::write(dir.join("20260726_120000.mp4"), b"").unwrap();
        write_sidecar(&dir, &sample_meta("20260726_120000")).unwrap();
        // An MP4 whose writer died before `Finalize`: `ftyp` and an open
        // `mdat`, no `moov`, no sidecar. The exact bytes Media Foundation left
        // when a 9:16 export was killed.
        let mut unfinished = Vec::new();
        unfinished.extend_from_slice(&[0, 0, 0, 0x18]);
        unfinished.extend_from_slice(b"ftypmp42\0\0\0\0mp41isom");
        unfinished.extend_from_slice(&[0, 0, 0, 1]);
        unfinished.extend_from_slice(b"mdat");
        unfinished.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0x10]);
        std::fs::write(dir.join("20260726_130000.mp4"), &unfinished).unwrap();
        // The same start, finalized: adopted.
        let mut finished = unfinished[..24].to_vec();
        finished.extend_from_slice(&[0, 0, 0, 8]);
        finished.extend_from_slice(b"moov");
        std::fs::write(dir.join("20260726_140000.mp4"), &finished).unwrap();

        let clips = scan(&dir).unwrap();
        let ids: Vec<&str> = clips.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["20260726_140000", "20260726_110000", "20260726_100000"], "newest first");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The ceiling deletes oldest-first and never touches a favorite, even
    /// when the favorites alone exceed it — a user who starred a clip has
    /// said "keep this", and silently deleting it is the one unrecoverable
    /// mistake this feature could make.
    #[test]
    fn the_ceiling_deletes_oldest_first_and_never_a_favorite() {
        let dir = std::env::temp_dir().join(format!("clippy-ceiling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Four clips of 1 GB each; the second-oldest is a favorite.
        let gb = 1_000_000_000u64;
        for (id, favorite) in [
            ("20260726_100000", false),
            ("20260726_110000", true),
            ("20260726_120000", false),
            ("20260726_130000", false),
        ] {
            std::fs::write(mp4_path(&dir, id), b"video").unwrap();
            let mut meta = sample_meta(id);
            meta.bytes = gb;
            meta.favorite = favorite;
            write_sidecar(&dir, &meta).unwrap();
        }

        // A 2 GB ceiling against 4 GB held: two must go.
        let deleted = prune_to_ceiling(&dir, 2).unwrap();
        assert_eq!(
            deleted,
            ["20260726_100000", "20260726_120000"],
            "oldest first, skipping the favorite"
        );
        assert!(!mp4_path(&dir, "20260726_100000").exists());
        assert!(!sidecar_path(&dir, "20260726_100000").exists(), "the sidecar goes with it");
        assert!(mp4_path(&dir, "20260726_110000").exists(), "a favorite is never deleted");
        assert!(mp4_path(&dir, "20260726_130000").exists(), "the newest survives");

        // A ceiling of 0 disables the feature outright.
        assert!(prune_to_ceiling(&dir, 0).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("clippy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_meta(id: &str) -> clippy_proto::ClipMeta {
        clippy_proto::ClipMeta {
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
    #[test]
    fn folder_names_are_cleaned_into_one_safe_component() {
        assert_eq!(sanitize_folder("VALORANT").as_deref(), Some("VALORANT"));
        assert_eq!(sanitize_folder("Counter-Strike 2").as_deref(), Some("Counter-Strike 2"));
        assert_eq!(
            sanitize_folder("Tom Clancy's: R6 / Siege?").as_deref(),
            Some("Tom Clancy's R6 Siege")
        );
        assert_eq!(sanitize_folder("  Game...  ").as_deref(), Some("Game"));
        assert_eq!(sanitize_folder("..\\..\\Windows").as_deref(), Some("Windows"));
        assert_eq!(sanitize_folder("???"), None);
        assert_eq!(
            sanitize_folder("Screenshots"),
            None,
            "that name is the screenshots' own folder"
        );
        assert_eq!(sanitize_folder("CON"), None);
        let long = "x".repeat(200);
        assert_eq!(sanitize_folder(&long).map(|f| f.len()), Some(MAX_FOLDER_LEN));
    }

    #[test]
    fn a_folder_that_could_leave_the_clips_folder_is_never_valid() {
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "C:",
            "..\\x",
            "x.",
            " x",
            "nul",
            "COM1",
            "lpt3.txt",
            "Screenshots",
            "a|b",
        ] {
            assert!(!is_valid_folder(bad), "{bad:?} must be rejected");
            assert!(folder_dir(Path::new("C:\\clips"), bad).is_none() || bad.is_empty());
        }
        for good in
            ["Bureau", "VALORANT", "Rocket League", "Chained Together", "Counter-strike 2", "COM0"]
        {
            assert!(is_valid_folder(good), "{good:?} must be accepted");
        }
        assert_eq!(folder_dir(Path::new("C:\\clips"), ""), Some(PathBuf::from("C:\\clips")));
    }

    #[test]
    fn scan_all_walks_the_clips_folder_and_one_level_of_game_folders() {
        let dir = scratch("scan-all");
        let valorant = dir.join("VALORANT");
        std::fs::create_dir_all(&valorant).unwrap();
        std::fs::create_dir_all(dir.join("Screenshots")).unwrap();
        std::fs::create_dir_all(valorant.join("deeper")).unwrap();
        for (d, id) in [
            (&dir, "20260101_100000"),
            (&valorant, "20260102_100000"),
            (&valorant.join("deeper"), "20260103_100000"),
        ] {
            std::fs::write(mp4_path(d, id), b"x").unwrap();
        }
        std::fs::write(mp4_path(&dir.join("Screenshots"), "20260104_100000"), b"x").unwrap();
        let all = scan_all(&dir).unwrap();
        let found: Vec<(&str, &str)> =
            all.iter().map(|c| (c.id.as_str(), c.folder.as_str())).collect();
        assert_eq!(found, [("20260102_100000", "VALORANT"), ("20260101_100000", "")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editor_cache_is_reserved_and_never_scanned_as_a_game() {
        let dir = scratch("reserved-cache");
        let cache = dir.join(TRACK_CACHE_DIR);
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(mp4_path(&cache, "20260101_100000"), b"not a library clip").unwrap();
        assert!(!is_valid_folder(TRACK_CACHE_DIR));
        assert!(!is_valid_folder(".CLIPPY-CACHE"));
        assert!(scan_all(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deleting_editor_audio_removes_cache_and_legacy_but_preserves_neighbors() {
        let dir = scratch("remove-track-cache");
        let cache = dir.join(TRACK_CACHE_DIR);
        std::fs::create_dir_all(&cache).unwrap();
        let id = "20260101_100000";
        for location in [&dir, &cache] {
            for suffix in ["t0.m4a", "t1.m4a", "t2.part.m4a"] {
                std::fs::write(location.join(format!("{id}.{suffix}")), b"derived audio").unwrap();
            }
            for name in
                [format!("{id}.mp4"), format!("{id}.title.m4a"), "20260101_100001.t0.m4a".into()]
            {
                std::fs::write(location.join(name), b"keep").unwrap();
            }
        }
        remove_track_files(&dir, id);
        for location in [&dir, &cache] {
            for suffix in ["t0.m4a", "t1.m4a", "t2.part.m4a"] {
                assert!(!location.join(format!("{id}.{suffix}")).exists());
            }
            for name in
                [format!("{id}.mp4"), format!("{id}.title.m4a"), "20260101_100001.t0.m4a".into()]
            {
                assert_eq!(std::fs::read(location.join(name)).unwrap(), b"keep");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_id_is_unique_across_folders() {
        let dir = scratch("alloc-in");
        let first = allocate_clip_id_in(&dir, "VALORANT").unwrap();
        let second = allocate_clip_id_in(&dir, "Bureau").unwrap();
        assert_ne!(first, second, "same-second clips in two folders must not share an id");
        assert!(mp4_path(&dir.join("VALORANT"), &first).exists());
        assert!(mp4_path(&dir.join("Bureau"), &second).exists());
        assert!(allocate_clip_id_in(&dir, "..").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_ceiling_counts_and_prunes_across_game_folders() {
        let dir = scratch("ceiling-folders");
        let game = dir.join("Rocket League");
        std::fs::create_dir_all(&game).unwrap();
        let old = "20260101_100000";
        std::fs::write(mp4_path(&game, old), vec![0u8; 3]).unwrap();
        std::fs::write(sidecar_path(&game, old), b"{").unwrap();
        std::fs::write(mp4_path(&dir, "20260102_100000"), vec![0u8; 3]).unwrap();
        // 1 GB ceiling cannot be exceeded by 6 bytes: nothing is deleted.
        assert!(prune_to_ceiling(&dir, 1).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
