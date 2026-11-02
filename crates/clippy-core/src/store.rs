//! The clip library's own records, kept out of the clips folder.
//!
//! The clips folder holds the user's videos and nothing else: one `.mp4` per
//! clip, in a sub-folder per game, named the way a person would ("VALORANT
//! 2026-11-02 14-27-22.mp4"), as the NVIDIA app and the Xbox Game Bar leave
//! them. What Clippy knows on top of the file lives here instead, the way
//! Medal keeps `%APPDATA%\Medal\store\clips.json`:
//!
//! - **The index**, `%APPDATA%\clippy\library.json`: per clip id, its file and
//!   what the file cannot say (favorite, game, track names, encoder, creation
//!   time with its offset). Ids stay `YYYYMMDD_HHMMSS[_N]` stamps, so the
//!   protocol and the UI keep their keys while the file is renamed freely.
//! - **The caches**, `%LOCALAPPDATA%\Clippy\cache`: thumbnails and the audio
//!   tracks the editor extracts. Losing them costs a re-extraction, nothing
//!   more.
//!
//! The title of a clip *is* its file name: renaming one renames the file.
//!
//! Every read-modify-write of the index goes through one process-wide lock;
//! the daemon is the only writer in practice.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result, bail};
use clippy_proto::ClipMeta;
use serde::{Deserialize, Serialize};

use crate::library::{self, DESKTOP_FOLDER, is_valid_id};
use crate::shot;

/// Where the library's records and caches live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    index: PathBuf,
    cache: PathBuf,
}

/// One record per clip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub clips: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// The clip's file, absolute.
    pub path: PathBuf,
    /// The file's modification time when last seen, in ms since 1970: with
    /// the size, how a file renamed or moved in Explorer is recognised.
    #[serde(default)]
    pub modified_ms: u64,
    #[serde(flatten)]
    pub meta: ClipMeta,
}

static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// Longest file name stem written; Windows allows more, but a path near
/// MAX_PATH breaks too many tools.
const MAX_STEM_LEN: usize = 120;

impl Store {
    pub fn new(index: PathBuf, cache: PathBuf) -> Self {
        Self { index, cache }
    }

    /// The user's own: `%APPDATA%\clippy\library.json` and
    /// `%LOCALAPPDATA%\Clippy\cache`.
    pub fn for_user() -> Option<Self> {
        let config = crate::config::Config::path()?;
        let cache = std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("Clippy").join("cache"))
            .unwrap_or_else(|| config.with_file_name("cache"));
        Some(Self::new(config.with_file_name("library.json"), cache))
    }

    /// Next to a config file other than the user's: a test's scratch config.
    pub fn beside(config: &Path) -> Self {
        Self::new(config.with_file_name("library.json"), config.with_file_name("cache"))
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }
    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache.join("thumbs")
    }
    pub fn tracks_dir(&self) -> PathBuf {
        self.cache.join("tracks")
    }
    pub fn shots_dir(&self) -> PathBuf {
        self.cache.join("shots")
    }
    /// Only for ids that passed [`is_valid_id`]; every caller checks first.
    pub fn thumb(&self, id: &str) -> PathBuf {
        self.thumbs_dir().join(format!("{id}.jpg"))
    }
    pub fn shot_thumb(&self, id: &str) -> PathBuf {
        self.shots_dir().join(format!("{id}.jpg"))
    }
    pub fn track(&self, id: &str, ordinal: usize) -> PathBuf {
        self.tracks_dir().join(format!("{id}.t{ordinal}.m4a"))
    }
    pub fn track_partial(&self, id: &str, ordinal: usize) -> PathBuf {
        self.tracks_dir().join(format!("{id}.t{ordinal}.part.m4a"))
    }

    /// The index as on disk; empty when there is none yet or it is unreadable
    /// (a broken index costs favorites and track names, never a clip).
    pub fn load(&self) -> Index {
        let _guard = lock();
        self.read()
    }

    fn read(&self) -> Index {
        match std::fs::read_to_string(&self.index) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %self.index.display(), error = %e, "unreadable library index; starting a new one");
                Index::default()
            }),
            Err(_) => Index::default(),
        }
    }

    fn write(&self, index: &Index) -> Result<()> {
        if let Some(parent) = self.index.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let temp = self.index.with_extension("json.tmp");
        let json = serde_json::to_string(index).context("sérialisation de la bibliothèque")?;
        std::fs::write(&temp, json).with_context(|| format!("écriture de {}", temp.display()))?;
        replace(&temp, &self.index)
    }

    /// Loads, applies `change`, and saves, under the index lock.
    pub fn update<R>(&self, change: impl FnOnce(&mut Index) -> R) -> Result<R> {
        let _guard = lock();
        let mut index = self.read();
        let result = change(&mut index);
        self.write(&index)?;
        Ok(result)
    }

    /// The entry for `id`, if the index has one, its title read off its file
    /// name (the title *is* the file name; the record's copy can be stale).
    pub fn entry(&self, id: &str) -> Option<Entry> {
        let mut entry = self.load().clips.get(id).cloned()?;
        entry.meta.id = id.to_string();
        if let Some(stem) = entry.path.file_stem() {
            entry.meta.title = stem.to_string_lossy().into_owned();
        }
        Some(entry)
    }

    /// Records a clip just written to `path`, returning the id it is filed
    /// under (`meta.id`, or the next free one after it).
    pub fn insert(&self, meta: &ClipMeta, path: &Path) -> Result<String> {
        self.update(|index| index.insert(meta.clone(), path))
    }

    pub fn write_thumb(&self, id: &str, jpeg: &[u8]) {
        let path = self.thumb(id);
        let written = std::fs::create_dir_all(self.thumbs_dir()).and_then(|()| std::fs::write(&path, jpeg));
        if let Err(e) = written {
            tracing::warn!(path = %path.display(), error = %e, "thumbnail not written");
        }
    }

    /// The thumbnail and extracted audio of `id`.
    pub fn remove_cached(&self, id: &str) {
        if !is_valid_id(id) {
            return;
        }
        remove_quietly(&self.thumb(id));
        let prefix = format!("{id}.t");
        let Ok(entries) = std::fs::read_dir(self.tracks_dir()) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(rest) = name.strip_prefix(&prefix) else { continue };
            let ordinal = rest.strip_suffix(".part.m4a").or_else(|| rest.strip_suffix(".m4a"));
            if ordinal.is_some_and(|o| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit())) {
                remove_quietly(&entry.path());
            }
        }
    }

    /// Renames the clip's file to `title` (its stem), in the same folder.
    /// Returns the title actually used, after cleaning.
    pub fn rename(&self, id: &str, title: &str) -> Result<String> {
        let stem = sanitize_file_stem(title).context("ce nom ne peut pas servir de nom de fichier")?;
        self.update(|index| -> Result<String> {
            let entry = index.clips.get_mut(id).with_context(|| format!("clip {id} introuvable"))?;
            let from = entry.path.clone();
            let dir = from.parent().context("dossier du clip")?;
            let to = dir.join(format!("{stem}.mp4"));
            if to == from {
                return Ok(stem);
            }
            let same_file = to.to_string_lossy().to_lowercase() == from.to_string_lossy().to_lowercase();
            if to.exists() && !same_file {
                bail!("un clip s’appelle déjà « {stem} » dans ce dossier");
            }
            std::fs::rename(&from, &to).map_err(|e| match e.raw_os_error() {
                // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
                Some(32 | 33) => anyhow::anyhow!("le clip est ouvert dans un autre programme ; fermez-le puis réessayez"),
                _ => anyhow::Error::new(e).context(format!("renommage de {}", from.display())),
            })?;
            entry.path = to;
            Ok(stem)
        })?
    }

    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        self.update(|index| -> Result<()> {
            let entry = index.clips.get_mut(id).with_context(|| format!("clip {id} introuvable"))?;
            entry.meta.favorite = favorite;
            Ok(())
        })?
    }

    /// Forgets `id`: its record and its caches. The file is the caller's.
    pub fn forget(&self, id: &str) -> Result<Option<Entry>> {
        let removed = self.update(|index| index.clips.remove(id))?;
        self.remove_cached(id);
        Ok(removed)
    }
}

impl Index {
    /// `stamp`, or `stamp_2`, `stamp_3`… whichever is free.
    pub fn free_id(&self, stamp: &str) -> String {
        if !self.clips.contains_key(stamp) {
            return stamp.to_string();
        }
        (2u32..)
            .map(|n| format!("{stamp}_{n}"))
            .find(|id| !self.clips.contains_key(id))
            .unwrap_or_else(|| stamp.to_string())
    }

    /// Files `meta` for the clip at `path` and returns its id: `meta.id` when
    /// free (or already this file's), otherwise the next free one after it.
    pub fn insert(&mut self, mut meta: ClipMeta, path: &Path) -> String {
        let stamp = if is_valid_id(&meta.id) { meta.id.clone() } else { library::now_id_stamp() };
        let mine = self.clips.get(&stamp).is_some_and(|e| same_path(&e.path, path));
        let id = if mine { stamp } else { self.free_id(base_stamp(&stamp)) };
        meta.id = id.clone();
        self.clips.insert(id.clone(), Entry { path: path.to_path_buf(), modified_ms: modified_ms(path), meta });
        id
    }
}

/// Every clip under the clips folder, newest first: the index reconciled
/// with what is on disk. Old-layout files (sidecars, thumbnails, the editor
/// cache) are moved out on the way; see [`migrate_dir`].
///
/// - a file the index knows is listed with its record, its size refreshed;
/// - a file it does not know, but whose size and modification time match a
///   record whose file is gone, is that clip renamed or moved in Explorer: it
///   keeps its favorite and track names;
/// - anything else is adopted, described by [`crate::export::probe`].
///
/// Records whose file is gone are dropped, but only while the clips folder is
/// there: an unplugged drive must not erase its clips' favorites.
pub fn scan_all(clip_dir: &Path, store: &Store) -> Result<Vec<ClipMeta>> {
    if let Err(e) = std::fs::read_dir(clip_dir) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(Vec::new());
        }
        return Err(e).with_context(|| format!("lecture de {}", clip_dir.display()));
    }
    let _guard = lock();
    let mut index = store.read();
    let mut changed = false;
    migrate_shots(&shot::shots_dir(clip_dir), store);

    let mut by_path: HashMap<String, String> =
        index.clips.iter().map(|(id, e)| (path_key(&e.path), id.clone())).collect();
    let mut listed = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (folder, dir) in library::folder_dirs(clip_dir) {
        changed |= migrate_dir(&dir, &folder, store, &mut index);
        if changed {
            by_path = index.clips.iter().map(|(id, e)| (path_key(&e.path), id.clone())).collect();
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            tracing::warn!(folder = %folder, "skipping an unreadable clips sub-folder");
            continue;
        };
        for file in entries.flatten() {
            let path = file.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let bytes = file.metadata().map(|m| m.len()).unwrap_or(1);
            // A reservation, or a writer that died before `Finalize`.
            if bytes == 0 || library::unfinished_mp4(&path) {
                continue;
            }
            let modified = modified_ms(&path);
            let id = match by_path.get(&path_key(&path)) {
                Some(id) => id.clone(),
                None => {
                    let moved = index
                        .clips
                        .iter()
                        .find(|(_, e)| e.meta.bytes == bytes && e.modified_ms == modified && !e.path.exists())
                        .map(|(id, _)| id.clone());
                    match moved {
                        Some(id) => {
                            tracing::info!(clip = %id, path = %path.display(), "found a clip renamed or moved outside Clippy");
                            id
                        }
                        None if clippy_named(&stem) => adopt(&path, &stem, &folder, store, &mut index),
                        // Someone else's video: the clips folder can be a whole
                        // drive shared with the NVIDIA app. Listing it would put
                        // it under the library ceiling, which deletes.
                        None => continue,
                    }
                }
            };
            changed = true;
            let Some(entry) = index.clips.get_mut(&id) else { continue };
            entry.path = path.clone();
            entry.modified_ms = modified;
            entry.meta.bytes = bytes;
            by_path.insert(path_key(&path), id.clone());
            seen.insert(id.clone());
            let mut meta = entry.meta.clone();
            meta.id = id;
            meta.title = stem;
            meta.folder = folder.clone();
            listed.push(meta);
        }
    }
    let root = path_key(clip_dir);
    let before = index.clips.len();
    index.clips.retain(|id, e| seen.contains(id) || !path_key(&e.path).starts_with(&root) || e.path.exists());
    changed |= index.clips.len() != before;
    if changed {
        if let Err(e) = store.write(&index) {
            tracing::warn!(error = %format!("{e:#}"), "library index not saved");
        }
    }
    listed.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(listed)
}

/// Whether a file name is one Clippy gives: a bare stamp (the old layout) or
/// "<name> YYYY-MM-DD HH-MM-SS", optionally " (n)". The NVIDIA app's
/// "Valorant 2026.02.21 - 21.55.38.02.DVR" is not.
fn clippy_named(stem: &str) -> bool {
    if is_valid_id(stem) {
        return true;
    }
    let stem = match stem.strip_suffix(')').and_then(|s| s.rsplit_once(" (")) {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => stem,
    };
    let Some((name, stamp)) = stem.len().checked_sub(20).and_then(|at| stem.is_char_boundary(at).then(|| stem.split_at(at))) else {
        return false;
    };
    let shape = stamp.bytes().enumerate().all(|(i, b)| match i {
        0 => b == b' ',
        5 | 8 | 14 | 17 => b == b'-',
        11 => b == b' ',
        _ => b.is_ascii_digit(),
    });
    shape && !name.trim().is_empty()
}

/// An `.mp4` the index has no record of: given an id (its own stem when that
/// is a free clip id, else one stamped from its modification time) and what
/// the file itself says.
fn adopt(path: &Path, stem: &str, folder: &str, store: &Store, index: &mut Index) -> String {
    let id = if is_valid_id(stem) && !index.clips.contains_key(stem) {
        stem.to_string()
    } else {
        index.free_id(&stamp_from_ms(modified_ms(path)))
    };
    let probe = crate::export::probe(path).unwrap_or_else(|e| {
        tracing::debug!(path = %path.display(), error = %format!("{e:#}"), "could not read a clip's headers");
        crate::export::Probe::default()
    });
    let game = if folder.is_empty() || folder == DESKTOP_FOLDER { String::new() } else { folder.to_string() };
    let meta = ClipMeta {
        id: id.clone(),
        title: stem.to_string(),
        created: library::created_from_id(&id),
        duration_ms: probe.duration_ms,
        bytes: 0,
        width: probe.width,
        height: probe.height,
        fps: probe.fps,
        encoder: String::new(),
        has_audio: probe.audio_tracks > 0,
        favorite: false,
        audio_tracks: (1..=probe.audio_tracks).map(|n| format!("Piste {n}")).collect(),
        game,
        folder: folder.to_string(),
    };
    // An old-layout thumbnail beside a clip that had no sidecar.
    if let Some(dir) = path.parent() {
        let old_thumb = dir.join(format!("{stem}.jpg"));
        if is_valid_id(stem) && old_thumb.is_file() {
            move_file(&old_thumb, &store.thumb(&id));
        }
    }
    index.clips.insert(id.clone(), Entry { path: path.to_path_buf(), modified_ms: modified_ms(path), meta });
    tracing::info!(clip = %id, path = %path.display(), "adopted a clip the library had no record of");
    id
}

/// Moves the old layout out of one clips folder: each `<id>.json` sidecar
/// becomes a record (and is deleted), its `<id>.jpg` goes to the thumbnail
/// cache, its `<id>.mp4` is renamed the new way, and `.clippy-cache` and
/// loose `<id>.tN.m4a` files (rebuildable) are deleted.
///
/// Safe to stop anywhere: records are saved before any file is renamed, and a
/// sidecar is deleted only after its clip is filed, so a later run finishes
/// the job without losing anything.
fn migrate_dir(dir: &Path, folder: &str, store: &Store, index: &mut Index) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    let mut sidecars = Vec::new();
    let mut leftovers = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".json") {
            if is_valid_id(id) {
                sidecars.push(id.to_string());
            }
        } else if name.strip_suffix(".json.tmp").is_some_and(is_valid_id) {
            leftovers.push(entry.path());
        } else if let Some(rest) = name.strip_suffix(".m4a") {
            // Loose editor audio from before `.clippy-cache`: `<id>.tN`.
            let rest = rest.strip_suffix(".part").unwrap_or(rest);
            if rest.rsplit_once(".t").is_some_and(|(id, n)| is_valid_id(id) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                leftovers.push(entry.path());
            }
        }
    }
    let old_cache = dir.join(".clippy-cache");
    if sidecars.is_empty() && leftovers.is_empty() && !old_cache.is_dir() {
        return false;
    }

    let mut renames = Vec::new();
    let mut taken: Vec<PathBuf> = Vec::new();
    for id in &sidecars {
        let sidecar = dir.join(format!("{id}.json"));
        let mp4 = library::mp4_path(dir, id);
        if !mp4.is_file() {
            // Its clip was deleted, or already renamed by an interrupted run
            // that filed it: either way nothing is left to keep. Only a file
            // that really is one of Clippy's sidecars goes.
            let ours = library::read_sidecar(&sidecar).is_ok_and(|meta| meta.id == *id);
            if ours && (!index.clips.contains_key(id) || index.clips.get(id).is_some_and(|e| e.path.exists())) {
                leftovers.push(sidecar);
                leftovers.push(dir.join(format!("{id}.jpg")));
            }
            continue;
        }
        let meta = match library::read_sidecar(&sidecar) {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(clip = %id, error = %format!("{e:#}"), "unreadable sidecar; the clip will be adopted");
                leftovers.push(sidecar);
                continue;
            }
        };
        let named = !meta.title.trim().is_empty() && meta.title != format!("clip_{id}");
        let stem = if named {
            sanitize_file_stem(&meta.title)
        } else {
            let name = if !meta.game.is_empty() { meta.game.as_str() } else if !folder.is_empty() { folder } else { DESKTOP_FOLDER };
            Some(clip_file_stem(name, id))
        }
        .unwrap_or_else(|| id.clone());
        let target = free_name(dir, &stem, &taken);
        taken.push(target.clone());
        let mut meta = meta;
        meta.id = id.clone();
        meta.folder = folder.to_string();
        meta.title = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        index.clips.insert(id.clone(), Entry { path: target.clone(), modified_ms: modified_ms(&mp4), meta });
        renames.push((id.clone(), mp4, target, sidecar));
    }
    // Records first: from here a crash costs nothing.
    if !renames.is_empty() {
        if let Err(e) = store.write(index) {
            tracing::warn!(error = %format!("{e:#}"), "library index not saved; leaving the old files for next time");
            return false;
        }
    }
    for (id, mp4, target, sidecar) in renames {
        if let Err(e) = std::fs::rename(&mp4, &target) {
            tracing::warn!(from = %mp4.display(), to = %target.display(), error = %e, "could not rename a clip; keeping its name");
            if let Some(entry) = index.clips.get_mut(&id) {
                entry.path = mp4.clone();
            }
        }
        let old_thumb = dir.join(format!("{id}.jpg"));
        if old_thumb.is_file() {
            move_file(&old_thumb, &store.thumb(&id));
        }
        remove_quietly(&sidecar);
        tracing::info!(clip = %id, "moved a clip to the new library layout");
    }
    for path in leftovers {
        remove_quietly(&path);
    }
    if let Ok(entries) = std::fs::read_dir(&old_cache) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().ends_with(".m4a") {
                remove_quietly(&entry.path());
            }
        }
        let _ = std::fs::remove_dir(&old_cache);
    }
    true
}

/// Screenshot thumbnails (`<id>.thumb.jpg` beside the image) move to the
/// cache.
fn migrate_shots(shots: &Path, store: &Store) {
    let Ok(entries) = std::fs::read_dir(shots) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".thumb.jpg") {
            if is_valid_id(id) {
                move_file(&entry.path(), &store.shot_thumb(id));
            }
        }
    }
}

/// Deletes the oldest non-favorite clips until the library fits under
/// `max_gb`, returning the ids removed, oldest first. `0` disables it.
///
/// Favorites are never deleted, even if they alone exceed the ceiling. A clip
/// whose file will not go is skipped and keeps counting.
pub fn prune_to_ceiling(clip_dir: &Path, store: &Store, max_gb: u32) -> Result<Vec<String>> {
    if max_gb == 0 {
        return Ok(Vec::new());
    }
    prune_to_bytes(clip_dir, store, u64::from(max_gb).saturating_mul(1_000_000_000))
}

fn prune_to_bytes(clip_dir: &Path, store: &Store, ceiling: u64) -> Result<Vec<String>> {
    let mut clips = scan_all(clip_dir, store)?;
    clips.reverse();
    let index = store.load();
    let mut held: u64 = clips.iter().map(|c| c.bytes).sum();
    let mut deleted = Vec::new();
    for clip in &clips {
        if held <= ceiling {
            break;
        }
        if clip.favorite {
            continue;
        }
        let Some(entry) = index.clips.get(&clip.id) else { continue };
        match std::fs::remove_file(&entry.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(clip = clip.id, error = %e, "ceiling could not delete a clip; keeping it and moving on");
                continue;
            }
        }
        if let Err(e) = store.forget(&clip.id) {
            tracing::warn!(clip = clip.id, error = %format!("{e:#}"), "ceiling deleted a clip but kept its record");
        }
        held = held.saturating_sub(clip.bytes);
        deleted.push(clip.id.clone());
    }
    if held > ceiling {
        tracing::warn!(held_bytes = held, ceiling_bytes = ceiling, "the clip library is over its ceiling and nothing left is safe to delete");
    }
    Ok(deleted)
}

/// « VALORANT 2026-11-02 14-27-22 » for `name` and the stamp `20261102_142722`.
pub fn clip_file_stem(name: &str, stamp: &str) -> String {
    let label = if stamp.len() >= 15 && is_valid_id(&stamp[..15]) {
        let (d, t) = (&stamp[..8], &stamp[9..15]);
        format!("{}-{}-{} {}-{}-{}", &d[..4], &d[4..6], &d[6..8], &t[..2], &t[2..4], &t[4..6])
    } else {
        stamp.to_string()
    };
    sanitize_file_stem(&format!("{name} {label}"))
        .or_else(|| sanitize_file_stem(&label))
        .unwrap_or_else(|| stamp.to_string())
}

/// `name` as one safe file name stem, or `None` when nothing usable is left.
/// Windows' forbidden and control characters go, whitespace collapses, and
/// trailing dots and spaces (which Windows strips) are removed; DOS device
/// names are refused.
pub fn sanitize_file_stem(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.trim_matches(|c| c == '.' || c == ' ').chars().take(MAX_STEM_LEN).collect();
    let stem = trimmed.trim_end_matches(|c| c == '.' || c == ' ').to_string();
    if stem.is_empty() {
        return None;
    }
    let lower = stem.to_lowercase();
    let base = lower.split('.').next().unwrap_or("");
    let numbered = |p: &str| base.strip_prefix(p).is_some_and(|n| n.len() == 1 && (b'1'..=b'9').contains(&n.as_bytes()[0]));
    if ["con", "prn", "aux", "nul"].contains(&base) || numbered("com") || numbered("lpt") {
        return None;
    }
    Some(stem)
}

/// Creates `dir\<stem>.mp4` empty and returns it, or `<stem> (2).mp4`… when
/// the name is taken. `create_new` makes the claim atomic, as for
/// [`library::allocate_clip_id`]; the caller owns the file from here on.
pub fn reserve_clip_file(dir: &Path, stem: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("impossible de créer le dossier des clips {}", dir.display()))?;
    for n in 1u32..10_000 {
        let name = if n == 1 { format!("{stem}.mp4") } else { format!("{stem} ({n}).mp4") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("impossible de réserver {}", path.display())),
        }
    }
    bail!("aucun nom de fichier libre pour {stem}")
}

/// The name a new clip from `game` (or the desktop) gets now.
pub fn new_clip_stem(game: &str, folder: &str, stamp: &str) -> String {
    let name = if !game.is_empty() {
        game
    } else if !folder.is_empty() {
        folder
    } else {
        DESKTOP_FOLDER
    };
    clip_file_stem(name, stamp)
}

/// `dir\<stem>.mp4`, or `<stem> (2).mp4`… not on disk nor in `taken`.
fn free_name(dir: &Path, stem: &str, taken: &[PathBuf]) -> PathBuf {
    (1u32..10_000)
        .map(|n| if n == 1 { dir.join(format!("{stem}.mp4")) } else { dir.join(format!("{stem} ({n}).mp4")) })
        .find(|p| !p.exists() && !taken.iter().any(|t| same_path(t, p)))
        .unwrap_or_else(|| dir.join(format!("{stem}.mp4")))
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    INDEX_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `20261102_142722_2` -> `20261102_142722`.
fn base_stamp(id: &str) -> &str {
    if id.len() > 15 { &id[..15] } else { id }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The local-time `YYYYMMDD_HHMMSS` stamp of a moment in ms since 1970.
fn stamp_from_ms(ms: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = ms * 10_000 + 116_444_736_000_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    let ok = unsafe {
        FileTimeToSystemTime(&file, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
    };
    if !ok {
        return library::now_id_stamp();
    }
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

/// Windows' rename refuses an existing destination; this one replaces it.
fn replace(from: &Path, to: &Path) -> Result<()> {
    let _ = std::fs::remove_file(to);
    std::fs::rename(from, to).with_context(|| format!("renommage vers {}", to.display()))
}

/// Rename, or copy then delete when `to` is on another drive (clips on E:,
/// cache on C:).
fn move_file(from: &Path, to: &Path) {
    if let Some(parent) = to.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(to);
    if std::fs::rename(from, to).is_ok() {
        return;
    }
    match std::fs::copy(from, to) {
        Ok(_) => remove_quietly(from),
        Err(e) => tracing::warn!(from = %from.display(), error = %e, "could not move a file to the library cache"),
    }
}

fn remove_quietly(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), error = %e, "could not remove a file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, Store) {
        let root = std::env::temp_dir().join(format!("clippy-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let clips = root.join("clips");
        std::fs::create_dir_all(&clips).unwrap();
        let store = Store::new(root.join("appdata").join("library.json"), root.join("local").join("cache"));
        (clips, store)
    }

    fn meta(id: &str) -> ClipMeta {
        ClipMeta {
            id: id.into(),
            title: format!("clip_{id}"),
            created: "2026-11-02T14:27:22+01:00".into(),
            duration_ms: 18_000,
            bytes: 5,
            width: 1920,
            height: 1080,
            fps: 120,
            encoder: "NVENC".into(),
            has_audio: true,
            favorite: false,
            audio_tracks: vec!["Mix".into(), "VALORANT".into(), "Micro".into()],
            game: "VALORANT".into(),
            folder: "VALORANT".into(),
        }
    }

    /// Lists what is left in a folder, names only, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        found.sort();
        found
    }

    #[test]
    fn the_old_layout_becomes_mp4s_only_and_keeps_everything_it_knew() {
        let (clips, store) = scratch("migrate");
        let game = clips.join("VALORANT");
        std::fs::create_dir_all(game.join(".clippy-cache")).unwrap();
        let id = "20261102_142722";
        std::fs::write(game.join(format!("{id}.mp4")), b"video").unwrap();
        let mut favorite = meta(id);
        favorite.favorite = true;
        library::write_sidecar(&game, &favorite).unwrap();
        std::fs::write(game.join(format!("{id}.jpg")), b"jpeg").unwrap();
        std::fs::write(game.join(".clippy-cache").join(format!("{id}.t1.m4a")), b"aac").unwrap();
        std::fs::write(game.join(format!("{id}.t2.m4a")), b"aac").unwrap();
        // A clip the user had named, at the root.
        let named = "20261101_100000";
        std::fs::write(clips.join(format!("{named}.mp4")), b"other").unwrap();
        let mut titled = meta(named);
        titled.title = "Mon ace: incroyable?".into();
        titled.game = String::new();
        library::write_sidecar(&clips, &titled).unwrap();
        // Screenshots keep their image; the thumbnail goes to the cache.
        let shots = clips.join("Screenshots");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::write(shots.join("20261102_150000.jpg"), b"shot").unwrap();
        std::fs::write(shots.join("20261102_150000.thumb.jpg"), b"thumb").unwrap();

        let listed = scan_all(&clips, &store).unwrap();

        assert_eq!(names(&game), ["VALORANT 2026-11-02 14-27-22.mp4"]);
        assert_eq!(names(&clips), ["Mon ace incroyable.mp4", "Screenshots", "VALORANT"]);
        assert_eq!(names(&shots), ["20261102_150000.jpg"]);
        assert_eq!(std::fs::read(store.shot_thumb("20261102_150000")).unwrap(), b"thumb");
        assert_eq!(std::fs::read(store.thumb(id)).unwrap(), b"jpeg");

        let clip = listed.iter().find(|c| c.id == id).unwrap();
        assert!(clip.favorite, "the favorite survives");
        assert_eq!(clip.audio_tracks, ["Mix", "VALORANT", "Micro"]);
        assert_eq!(clip.title, "VALORANT 2026-11-02 14-27-22");
        assert_eq!(clip.folder, "VALORANT");
        assert_eq!(listed.iter().find(|c| c.id == named).unwrap().title, "Mon ace incroyable");

        // A second pass changes nothing.
        let again = scan_all(&clips, &store).unwrap();
        assert_eq!(again, listed);
        assert_eq!(names(&game), ["VALORANT 2026-11-02 14-27-22.mp4"]);
    }

    #[test]
    fn a_clip_renamed_or_moved_in_explorer_keeps_its_record() {
        let (clips, store) = scratch("reattach");
        let path = clips.join("Bureau 2026-11-02 14-27-22.mp4");
        std::fs::write(&path, b"video").unwrap();
        let id = store.insert(&ClipMeta { favorite: true, bytes: 5, ..meta("20261102_142722") }, &path).unwrap();
        let moved = clips.join("Sub");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::rename(&path, moved.join("renamed.mp4")).unwrap();

        let listed = scan_all(&clips, &store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].favorite);
        assert_eq!((listed[0].title.as_str(), listed[0].folder.as_str()), ("renamed", "Sub"));
    }

    #[test]
    fn an_unknown_mp4_is_adopted_and_a_deleted_one_forgotten() {
        let (clips, store) = scratch("adopt");
        std::fs::write(clips.join("Bureau 2026-01-02 10-00-00.mp4"), b"not really a video").unwrap();
        std::fs::write(clips.join("20260101_100000.mp4"), b"x").unwrap();
        std::fs::write(clips.join("reserved.mp4"), b"").unwrap();
        let listed = scan_all(&clips, &store).unwrap();
        assert_eq!(listed.len(), 2, "a zero-byte reservation is not a clip");
        assert!(listed.iter().any(|c| c.id == "20260101_100000"), "a stamp name keeps its id");
        assert!(listed.iter().all(|c| is_valid_id(&c.id)));

        std::fs::remove_file(clips.join("Bureau 2026-01-02 10-00-00.mp4")).unwrap();
        assert_eq!(scan_all(&clips, &store).unwrap().len(), 1);
        assert_eq!(store.load().clips.len(), 1, "the deleted clip's record is dropped");
    }

    #[test]
    fn an_unplugged_clips_folder_keeps_its_records() {
        let (clips, store) = scratch("unplugged");
        let path = clips.join("a.mp4");
        std::fs::write(&path, b"x").unwrap();
        store.insert(&meta("20260101_100000"), &path).unwrap();
        std::fs::remove_dir_all(&clips).unwrap();
        assert!(scan_all(&clips, &store).unwrap().is_empty());
        assert_eq!(store.load().clips.len(), 1);
    }

    #[test]
    fn renaming_renames_the_file_and_refuses_a_clash() {
        let (clips, store) = scratch("rename");
        let path = clips.join("VALORANT 2026-11-02 14-27-22.mp4");
        std::fs::write(&path, b"x").unwrap();
        std::fs::write(clips.join("pris.mp4"), b"y").unwrap();
        let id = store.insert(&meta("20261102_142722"), &path).unwrap();
        assert_eq!(store.rename(&id, "  Mon / ace  ").unwrap(), "Mon ace");
        assert!(clips.join("Mon ace.mp4").is_file());
        assert!(!path.exists());
        assert!(store.rename(&id, "pris").is_err(), "an existing name is never overwritten");
        assert_eq!(store.rename(&id, "MON ACE").unwrap(), "MON ACE", "a change of case is a rename");
        assert!(store.rename(&id, "???").is_err());
        assert_eq!(store.entry(&id).unwrap().path, clips.join("MON ACE.mp4"));
        assert_eq!(store.entry(&id).unwrap().meta.title, "MON ACE", "an export is named after this");
    }

    #[test]
    fn new_clips_are_named_after_the_game_and_never_overwrite() {
        assert_eq!(new_clip_stem("VALORANT", "VALORANT", "20261102_142722"), "VALORANT 2026-11-02 14-27-22");
        assert_eq!(new_clip_stem("", "", "20261102_142722_2"), "Bureau 2026-11-02 14-27-22");
        assert_eq!(new_clip_stem("Tom Clancy's: R6?", "x", "20261102_142722"), "Tom Clancy's R6 2026-11-02 14-27-22");
        let (clips, _) = scratch("reserve");
        let first = reserve_clip_file(&clips, "Bureau 2026-11-02 14-27-22").unwrap();
        let second = reserve_clip_file(&clips, "Bureau 2026-11-02 14-27-22").unwrap();
        assert_eq!(first.file_name().unwrap(), "Bureau 2026-11-02 14-27-22.mp4");
        assert_eq!(second.file_name().unwrap(), "Bureau 2026-11-02 14-27-22 (2).mp4");
    }

    /// `CLIPPY_STORE_REAL=<clips dir>`: scans a real folder (a copy!) with a
    /// store beside it and prints what the library becomes.
    #[test]
    #[ignore]
    fn real_folder() {
        let dir = PathBuf::from(std::env::var("CLIPPY_STORE_REAL").expect("CLIPPY_STORE_REAL"));
        let store = Store::new(dir.with_extension("library.json"), dir.with_extension("cache"));
        for clip in scan_all(&dir, &store).unwrap() {
            println!("{} | {} | {} | {} ms {}x{}@{} | fav={} | tracks={:?}", clip.id, clip.folder, clip.title, clip.duration_ms, clip.width, clip.height, clip.fps, clip.favorite, clip.audio_tracks);
        }
    }

    #[test]
    fn only_clippy_names_are_adopted() {
        assert!(clippy_named("20261102_142722"));
        assert!(clippy_named("VALORANT 2026-11-02 14-27-22"));
        assert!(clippy_named("Bureau 2026-11-02 14-27-22 (2)"));
        assert!(clippy_named("Counter-Strike 2 2026-11-02 14-27-22"));
        assert!(!clippy_named("Valorant 2026.02.21 - 21.55.38.02.DVR"));
        assert!(!clippy_named("Desktop 2025.04.03 - 21.50.50.02"));
        assert!(!clippy_named("coridor"));
        assert!(!clippy_named(" 2026-11-02 14-27-22"));
        assert!(!clippy_named("Mon ace"));

        let (clips, store) = scratch("foreign");
        let nvidia = clips.join("Valorant");
        std::fs::create_dir_all(&nvidia).unwrap();
        std::fs::write(nvidia.join("Valorant 2026.02.21 - 21.55.38.02.DVR.mp4"), b"theirs").unwrap();
        // Not a sidecar of ours: left alone.
        std::fs::write(nvidia.join("20260221_215538.json"), b"{\"not\": \"ours\"}").unwrap();
        std::fs::write(nvidia.join("notes.json.tmp"), b"x").unwrap();
        assert!(scan_all(&clips, &store).unwrap().is_empty(), "another app's clips are not listed");
        assert_eq!(names(&nvidia).len(), 3, "and nothing of theirs is touched");
        assert!(store.load().clips.is_empty());
    }

    #[test]
    fn file_stems_are_cleaned() {
        assert_eq!(sanitize_file_stem("a:b?c").as_deref(), Some("abc"));
        assert_eq!(sanitize_file_stem("  x.  ").as_deref(), Some("x"));
        assert_eq!(sanitize_file_stem("nul"), None);
        assert_eq!(sanitize_file_stem("COM1"), None);
        assert_eq!(sanitize_file_stem("..\\..\\x").as_deref(), Some("x"));
        assert_eq!(sanitize_file_stem(&"y".repeat(300)).map(|s| s.len()), Some(MAX_STEM_LEN));
    }

    #[test]
    fn the_ceiling_deletes_oldest_first_and_never_a_favorite() {
        let (clips, store) = scratch("ceiling");
        let gb = 1_000_000_000u64;
        for (id, favorite) in [
            ("20260726_100000", false),
            ("20260726_110000", true),
            ("20260726_120000", false),
            ("20260726_130000", false),
        ] {
            let path = clips.join(format!("{id}.mp4"));
            std::fs::write(&path, b"video").unwrap();
            store.insert(&ClipMeta { favorite, bytes: 5, ..meta(id) }, &path).unwrap();
            store.write_thumb(id, b"jpeg");
        }
        assert!(prune_to_ceiling(&clips, &store, 1).unwrap().is_empty(), "20 bytes fit in 1 GB");
        assert!(prune_to_ceiling(&clips, &store, 0).unwrap().is_empty(), "0 disables the ceiling");
        let _ = gb;
        // 20 bytes held, 10 allowed: the two oldest non-favorites go.
        let removed = prune_to_bytes(&clips, &store, 10).unwrap();
        assert_eq!(removed, ["20260726_100000", "20260726_120000"]);
        assert!(!store.thumb("20260726_100000").exists(), "its thumbnail goes too");
        assert!(store.thumb("20260726_130000").exists());
        assert_eq!(names(&clips), ["20260726_110000.mp4", "20260726_130000.mp4"]);
        // Only the favorite would be left to delete: it stays.
        assert_eq!(prune_to_bytes(&clips, &store, 1).unwrap(), ["20260726_130000"]);
        assert_eq!(names(&clips), ["20260726_110000.mp4"]);
    }
}
