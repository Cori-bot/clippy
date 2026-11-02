//! The games Clippy knows: the ones it noticed running, the ones the user
//! added, and what each one records.
//!
//! Stored in `games.json` next to `config.toml`. This is data the daemon
//! writes as the user plays, unlike the config, which the user edits, so a
//! detected game never rewrites their settings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most programs a profile may add on top of the game itself.
const MAX_PROFILE_APPS: usize = 8;
const MAX_NAME_CHARS: usize = 64;

/// A program whose sound goes in the clip as a track of its own.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProfileApp {
    pub exe: String,
    #[serde(default)]
    pub name: String,
}

/// What a game's clips carry besides the game's own sound.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GameProfile {
    #[serde(default)]
    pub apps: Vec<ProfileApp>,
    /// Also record the microphone, as its own track.
    #[serde(default)]
    pub mic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEntry {
    /// Lower-case file name of the program, the identity of the game.
    pub exe: String,
    pub name: String,
    /// Off means "never treat this program as a game", even full screen.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Added by the user, so it counts as a game even windowed.
    #[serde(default)]
    pub manual: bool,
    /// Local RFC 3339 time it was last seen running.
    #[serde(default)]
    pub last_played: Option<String>,
    #[serde(default)]
    pub profile: GameProfile,
    /// Removed by the user although Clippy detected it: kept, switched off and
    /// out of the list, so it is never treated as a game nor detected again
    /// until the user adds it back.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize, Deserialize, Default)]
struct FileShape {
    #[serde(default)]
    games: Vec<GameEntry>,
}

/// `chrome.exe` for `C:\x\Chrome.EXE`; `None` for anything that is not a bare
/// program file name. The registry is keyed by this, and it ends up in a
/// config-like file and a socket reply, so nothing path-shaped gets in.
pub fn normalize_exe(raw: &str) -> Option<String> {
    let exe = raw.trim().to_lowercase();
    let ok = exe.len() > 4
        && exe.len() <= 128
        && exe.ends_with(".exe")
        && !exe.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        && !exe.chars().any(char::is_control);
    ok.then_some(exe)
}

fn clean_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// `eldenring.exe` -> `Eldenring`: the fallback label for a program that has
/// no product name of its own.
pub fn name_from_exe(exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl GameProfile {
    /// Drops what cannot be recorded: bad names, repeats, the game itself, and
    /// anything past the cap.
    fn cleaned(mut self, game_exe: &str) -> Self {
        let mut seen: Vec<String> = Vec::new();
        self.apps = self
            .apps
            .into_iter()
            .filter_map(|app| {
                let exe = normalize_exe(&app.exe)?;
                if exe == game_exe || seen.contains(&exe) {
                    return None;
                }
                seen.push(exe.clone());
                let name = clean_name(&app.name);
                Some(ProfileApp {
                    name: if name.is_empty() { name_from_exe(&exe) } else { name },
                    exe,
                })
            })
            .take(MAX_PROFILE_APPS)
            .collect();
        self
    }
}

/// The edits `games.set` can make; `None` leaves a field alone.
#[derive(Default)]
pub struct Edit {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub profile: Option<GameProfile>,
}

#[derive(Default)]
pub struct Registry {
    path: Option<PathBuf>,
    games: Vec<GameEntry>,
}

impl Registry {
    /// Reads `path`; a missing or unreadable file is an empty registry (the
    /// next change writes a fresh one), and `None` keeps it in memory only, for
    /// tests.
    pub fn load(path: Option<PathBuf>) -> Self {
        let games = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| match serde_json::from_str::<FileShape>(&text) {
                Ok(file) => Some(file.games),
                Err(e) => {
                    tracing::warn!(error = %e, "games.json is unreadable; starting with no games");
                    None
                }
            })
            .unwrap_or_default();
        let mut registry = Self { path, games: Vec::new() };
        let mut dropped = false;
        // Re-validated on the way in: the file is user-editable.
        for mut entry in games {
            let Some(exe) = normalize_exe(&entry.exe) else { continue };
            if registry.games.iter().any(|g| g.exe == exe) {
                continue;
            }
            // Detected by an older Clippy that took any full-screen window for
            // a game (the screen-capture overlay, a photo viewer...).
            if !entry.manual && !entry.hidden && crate::game::excluded(&exe, &exe) {
                dropped = true;
                continue;
            }
            entry.profile = entry.profile.cleaned(&exe);
            entry.exe = exe;
            let name = clean_name(&entry.name);
            entry.name = if name.is_empty() { name_from_exe(&entry.exe) } else { name };
            registry.games.push(entry);
        }
        if dropped {
            registry.save();
        }
        registry
    }

    /// Writes the file; failure is logged, not fatal -- losing the list is a
    /// nuisance, losing the replay is not.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let shape = FileShape { games: self.games.clone() };
        let result = serde_json::to_string_pretty(&shape)
            .map_err(|e| e.to_string())
            .and_then(|text| write_atomically(path, &text).map_err(|e| e.to_string()));
        if let Err(e) = result {
            tracing::warn!(path = %path.display(), error = %e, "could not save the games list");
        }
    }

    /// Most recently played first, then the rest by name.
    pub fn list(&self) -> Vec<GameEntry> {
        let mut games: Vec<GameEntry> = self.games.iter().filter(|g| !g.hidden).cloned().collect();
        games.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        games
    }

    pub fn find(&self, exe: &str) -> Option<&GameEntry> {
        self.games.iter().find(|g| g.exe == exe)
    }

    /// Programs that count as games even windowed.
    pub fn manual_exes(&self) -> Vec<String> {
        self.games
            .iter()
            .filter(|g| g.enabled && g.manual && !g.hidden)
            .map(|g| g.exe.clone())
            .collect()
    }

    /// Programs the user switched off.
    pub fn disabled_exes(&self) -> Vec<String> {
        self.games.iter().filter(|g| !g.enabled || g.hidden).map(|g| g.exe.clone()).collect()
    }

    /// Adds the installed games the registry does not have yet, `(exe, name)`
    /// pairs, as detected entries never played. A game the user removed stays
    /// removed. Returns how many were added.
    pub fn add_installed(&mut self, found: &[(String, String)]) -> usize {
        let mut added = 0;
        for (exe, name) in found {
            let Some(exe) = normalize_exe(exe) else { continue };
            if self.games.iter().any(|g| g.exe == exe) {
                continue;
            }
            let name = clean_name(name);
            self.games.push(GameEntry {
                name: if name.is_empty() { name_from_exe(&exe) } else { name },
                exe,
                enabled: true,
                manual: false,
                last_played: None,
                profile: GameProfile::default(),
                hidden: false,
            });
            added += 1;
        }
        if added > 0 {
            self.save();
        }
        added
    }

    /// Records that `exe` is running now. Adds it on first sight. Returns
    /// whether anything changed, so the caller knows to tell the UI.
    pub fn observe(&mut self, exe: &str, name: &str, now: &str) -> bool {
        let Some(exe) = normalize_exe(exe) else { return false };
        match self.games.iter_mut().find(|g| g.exe == exe) {
            Some(entry) => {
                if entry.hidden || entry.last_played.as_deref() == Some(now) {
                    return false;
                }
                entry.last_played = Some(now.to_string());
            }
            None => {
                let name = clean_name(name);
                self.games.push(GameEntry {
                    name: if name.is_empty() { name_from_exe(&exe) } else { name },
                    exe,
                    enabled: true,
                    manual: false,
                    last_played: Some(now.to_string()),
                    profile: GameProfile::default(),
                    hidden: false,
                });
            }
        }
        self.save();
        true
    }

    /// Applies `edit` to `exe`, creating a manual entry for a program the
    /// registry has not seen (the "add a game" button).
    pub fn set(&mut self, exe: &str, edit: Edit, fallback_name: &str) -> Result<GameEntry, String> {
        let exe =
            normalize_exe(exe).ok_or_else(|| format!("« {exe} » n’est pas un programme (.exe)"))?;
        let at = match self.games.iter().position(|g| g.exe == exe) {
            Some(at) => at,
            None => {
                let name = clean_name(fallback_name);
                self.games.push(GameEntry {
                    name: if name.is_empty() { name_from_exe(&exe) } else { name },
                    exe: exe.clone(),
                    enabled: true,
                    manual: true,
                    last_played: None,
                    profile: GameProfile::default(),
                    hidden: false,
                });
                self.games.len() - 1
            }
        };
        let entry = &mut self.games[at];
        // Adding back a game the user removed.
        if entry.hidden {
            entry.hidden = false;
            entry.enabled = true;
            entry.manual = true;
        }
        if let Some(enabled) = edit.enabled {
            entry.enabled = enabled;
        }
        if let Some(name) = edit.name {
            let name = clean_name(&name);
            if !name.is_empty() {
                entry.name = name;
            }
        }
        if let Some(profile) = edit.profile {
            entry.profile = profile.cleaned(&exe);
        }
        let out = entry.clone();
        self.save();
        Ok(out)
    }

    /// Removes a game from the list. One the user added is forgotten; one
    /// Clippy detected is hidden and switched off instead, so it neither comes
    /// straight back nor counts as a game until the user adds it again.
    pub fn remove(&mut self, exe: &str) -> bool {
        let Some(exe) = normalize_exe(exe) else { return false };
        let Some(at) = self.games.iter().position(|g| g.exe == exe && !g.hidden) else {
            return false;
        };
        // Hidden rather than deleted, for a game added by hand too: an
        // installed one would otherwise be added straight back by the next
        // scan. Adding it again (`set`) brings it back.
        let entry = &mut self.games[at];
        entry.hidden = true;
        entry.enabled = false;
        self.save();
        true
    }
}

fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::load(None)
    }

    #[test]
    fn only_a_bare_program_name_is_an_identity() {
        assert_eq!(normalize_exe("  Valorant.EXE "), Some("valorant.exe".into()));
        for bad in
            ["", ".exe", "a", "game", "C:\\x\\game.exe", "../game.exe", "a/b.exe", "x.exe:ads"]
        {
            assert_eq!(normalize_exe(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn seeing_a_program_adds_it_once_and_refreshes_its_time() {
        let mut r = registry();
        assert!(r.observe("VALORANT.exe", "VALORANT", "2026-10-20T10:00:00+02:00"));
        assert!(
            !r.observe("valorant.exe", "VALORANT", "2026-10-20T10:00:00+02:00"),
            "same stamp, no change"
        );
        assert!(r.observe("valorant.exe", "ignored", "2026-10-20T10:01:00+02:00"));
        let games = r.list();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "VALORANT", "the name given on first sight stays");
        assert!(games[0].enabled && !games[0].manual);
        assert_eq!(games[0].last_played.as_deref(), Some("2026-10-20T10:01:00+02:00"));
    }

    #[test]
    fn adding_by_hand_makes_a_manual_entry_and_edits_keep_the_rest() {
        let mut r = registry();
        let added = r.set("minecraft.exe", Edit::default(), "Minecraft").unwrap();
        assert!(added.manual && added.enabled && added.last_played.is_none());
        assert_eq!(r.manual_exes(), ["minecraft.exe"]);
        let off =
            r.set("minecraft.exe", Edit { enabled: Some(false), ..Edit::default() }, "").unwrap();
        assert!(!off.enabled && off.name == "Minecraft" && off.manual);
        assert!(r.manual_exes().is_empty(), "a switched-off game is not a manual one");
        assert_eq!(r.disabled_exes(), ["minecraft.exe"]);
        assert!(r.set("not a program", Edit::default(), "").is_err());
    }

    #[test]
    fn a_profile_is_cleaned_of_repeats_the_game_itself_and_junk() {
        let mut r = registry();
        let profile = GameProfile {
            apps: vec![
                ProfileApp { exe: "Discord.exe".into(), name: " Discord ".into() },
                ProfileApp { exe: "discord.exe".into(), name: "again".into() },
                ProfileApp { exe: "valorant.exe".into(), name: "itself".into() },
                ProfileApp { exe: "..\\evil".into(), name: "x".into() },
                ProfileApp { exe: "spotify.exe".into(), name: String::new() },
            ],
            mic: true,
        };
        let entry = r
            .set("valorant.exe", Edit { profile: Some(profile), ..Edit::default() }, "Valorant")
            .unwrap();
        let apps: Vec<(&str, &str)> =
            entry.profile.apps.iter().map(|a| (a.exe.as_str(), a.name.as_str())).collect();
        assert_eq!(apps, [("discord.exe", "Discord"), ("spotify.exe", "Spotify")]);
        assert!(entry.profile.mic);
    }

    #[test]
    fn removing_a_detected_game_hides_it_until_it_is_added_back() {
        let mut r = registry();
        r.observe("a.exe", "A", "t");
        assert!(r.remove("A.exe"));
        assert!(!r.remove("a.exe"), "already removed");
        assert!(r.list().is_empty());
        assert_eq!(r.disabled_exes(), ["a.exe"], "never a game while hidden");
        assert!(!r.observe("a.exe", "A", "t2"), "seeing it again does not bring it back");
        assert_eq!(r.add_installed(&[("a.exe".into(), "A".into())]), 0, "nor does a scan");
        assert!(r.list().is_empty());
        let back = r.set("a.exe", Edit::default(), "").unwrap();
        assert!(back.manual && back.enabled && !back.hidden);
        assert_eq!(r.list().len(), 1);
    }

    #[test]
    fn removing_a_game_added_by_hand_hides_it_for_good() {
        let mut r = registry();
        r.set("m.exe", Edit::default(), "M").unwrap();
        assert!(r.remove("m.exe"));
        assert!(r.list().is_empty());
        assert!(r.manual_exes().is_empty(), "no longer a game");
        r.add_installed(&[("m.exe".to_string(), "M".to_string())]);
        assert!(r.list().is_empty(), "a scan does not bring it back");
    }

    #[test]
    fn installed_games_are_added_once_never_played() {
        let mut r = registry();
        r.observe("cs2.exe", "cs2", "t");
        let found = [
            ("cs2.exe".to_string(), "Counter-Strike 2".to_string()),
            ("AimLab_tb.exe".to_string(), "Aim Lab".to_string()),
        ];
        assert_eq!(r.add_installed(&found), 1);
        assert_eq!(r.add_installed(&found), 0);
        let aim = r.find("aimlab_tb.exe").unwrap();
        assert!(!aim.manual && aim.enabled && aim.last_played.is_none());
        assert_eq!(aim.name, "Aim Lab");
        assert_eq!(r.find("cs2.exe").unwrap().name, "cs2", "an existing entry is left alone");
    }

    #[test]
    fn a_detected_non_game_is_dropped_on_load() {
        let dir = std::env::temp_dir().join(format!("clippy-games-drop-{}", std::process::id()));
        let path = dir.join("games.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{"games":[{"exe":"snippingtool.exe","name":"Snip"},{"exe":"photos.exe","name":"P","manual":true},{"exe":"valorant-win64-shipping.exe","name":"VALORANT"}]}"#,
        )
        .unwrap();
        let r = Registry::load(Some(path.clone()));
        let exes: Vec<_> = r.list().into_iter().map(|g| g.exe).collect();
        assert_eq!(exes, ["photos.exe", "valorant-win64-shipping.exe"], "added by hand stays");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("snippingtool"), "saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_puts_the_latest_game_first() {
        let mut r = registry();
        r.observe("old.exe", "Old", "2026-01-01T00:00:00+00:00");
        r.observe("new.exe", "New", "2026-02-01T00:00:00+00:00");
        r.set("never.exe", Edit::default(), "Never").unwrap();
        let order: Vec<_> = r.list().into_iter().map(|g| g.exe).collect();
        assert_eq!(order, ["new.exe", "old.exe", "never.exe"]);
    }

    #[test]
    fn the_file_round_trips_and_a_hand_edited_one_is_re_validated() {
        let dir = std::env::temp_dir().join(format!("clippy-games-{}", std::process::id()));
        let path = dir.join("games.json");
        let mut r = Registry::load(Some(path.clone()));
        r.set(
            "valorant.exe",
            Edit { profile: Some(GameProfile { apps: vec![], mic: true }), ..Edit::default() },
            "Valorant",
        )
        .unwrap();
        let again = Registry::load(Some(path.clone()));
        assert_eq!(again.list(), r.list());
        std::fs::write(&path, r#"{"games":[{"exe":"..\\x","name":"bad"},{"exe":"OK.exe","name":"  "},{"exe":"ok.exe","name":"dup"}]}"#).unwrap();
        let fixed = Registry::load(Some(path));
        let names: Vec<_> = fixed.list().into_iter().map(|g| (g.exe, g.name)).collect();
        assert_eq!(names, [("ok.exe".to_string(), "Ok".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
