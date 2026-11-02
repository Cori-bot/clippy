//! The games installed on this PC, read from what the stores and Windows keep.
//!
//! A full-screen window alone does not make a game: a photo viewer, a video or
//! a screen-capture overlay fill the screen too. What makes one is that a
//! store installed it, or that Windows' own Game Bar recognised it. This module
//! gathers both, best effort -- every source may be missing or unreadable, and
//! none stops the others:
//!
//! - `HKCU\System\GameConfigStore\Children\*\MatchedExeFullPath`: programs
//!   Windows matched as games (exact files);
//! - Steam: `steamapps\appmanifest_*.acf` in every library (install folders);
//! - Epic: `*.item` manifests (install folder and the program it launches);
//! - Riot: `*.product_settings.yaml` (install folder; the real game program
//!   for VALORANT and League of Legends);
//! - GOG, Ubisoft, EA, Battle.net: their registry keys (install folders);
//! - `C:\XboxGames\*` (install folders).
//!
//! Scanned at startup and every ten minutes; the result is shared by the game
//! watcher (is this window a game?) and `games.scan` (list them).

use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::core::{HSTRING, PWSTR};

/// What is known to be installed. Paths are lower-case, with backslashes and
/// no trailing separator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    /// Game programs, by full path, with the game's name.
    exes: Vec<(String, String)>,
    /// Game install folders, with the game's name.
    roots: Vec<(String, String)>,
}

static CURRENT: RwLock<Installed> = RwLock::new(Installed { exes: Vec::new(), roots: Vec::new() });

/// The last scan's result.
pub fn current() -> RwLockReadGuard<'static, Installed> {
    CURRENT.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Scans now and makes the result current.
pub fn rescan() {
    let found = scan();
    tracing::info!(
        programs = found.exes.len(),
        folders = found.roots.len(),
        "installed games scanned"
    );
    *CURRENT.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
}

/// `C:/Games/X/` -> `c:\games\x`.
fn norm(path: &str) -> String {
    let p = path.trim().trim_matches('"').replace('/', "\\").to_lowercase();
    p.trim_end_matches('\\').to_string()
}

/// "Rocket League®" -> "Rocket League".
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '®' | '™')).collect::<String>().trim().to_string()
}

impl Installed {
    pub fn add_exe(&mut self, path: &str, name: &str) {
        let path = norm(path);
        let name = clean_name(name);
        if path.is_empty() || !path.ends_with(".exe") {
            return;
        }
        match self.exes.iter_mut().find(|(p, _)| *p == path) {
            // Windows' own list has no names; a store's fills them in.
            Some((_, known)) if known.is_empty() => *known = name,
            Some(_) => {}
            None => self.exes.push((path, name)),
        }
    }

    pub fn add_root(&mut self, path: &str, name: &str) {
        let path = norm(path);
        // A drive root, or a first-level folder like `c:\games` or
        // `c:\program files`, would make everything under it a game.
        if path.matches('\\').count() < 2 || self.roots.iter().any(|(p, _)| *p == path) {
            return;
        }
        self.roots.push((path, clean_name(name)));
    }

    fn root_of(&self, path: &str) -> Option<&str> {
        self.roots
            .iter()
            .filter(|(root, _)| {
                path.len() > root.len()
                    && path.starts_with(root.as_str())
                    && path.as_bytes()[root.len()] == b'\\'
            })
            .max_by_key(|(root, _)| root.len())
            .map(|(_, name)| name.as_str())
    }

    /// Whether `path` is an installed game: its name (empty when unknown), and
    /// whether it is the game's own program (`true`) or only a program in its
    /// install folder.
    pub fn classify(&self, path: &str) -> Option<(String, bool)> {
        let path = norm(path);
        let root = self.root_of(&path);
        if let Some((_, name)) = self.exes.iter().find(|(p, _)| *p == path) {
            let name = if name.is_empty() { root.unwrap_or("") } else { name };
            return Some((name.to_string(), true));
        }
        root.map(|name| (name.to_string(), false))
    }

    /// The game programs to list: `(full path, name)`, programs that exist
    /// and are not excluded. The name may be empty (unknown).
    pub fn games(&self) -> Vec<(String, String)> {
        self.exes
            .iter()
            .filter(|(path, _)| {
                let exe = path.rsplit('\\').next().unwrap_or(path);
                !crate::game::excluded(path, exe) && Path::new(path).is_file()
            })
            .map(|(path, name)| {
                let name = if name.is_empty() { self.root_of(path).unwrap_or("") } else { name };
                (path.clone(), name.to_string())
            })
            .collect()
    }
}

// ------------------------------------------------------------------ parsers

/// `"key"  "value"` pairs of a Valve KeyValues file, top level or not.
fn vdf_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut quoted = line.split('"').skip(1).step_by(2);
        (quoted.next()?.eq_ignore_ascii_case(key))
            .then(|| quoted.next().map(|v| v.replace("\\\\", "\\")))
            .flatten()
    })
}

/// Steam tools that are not games.
const STEAM_TOOL_APPIDS: &[&str] = &["228980", "1070560", "1391110", "1628350", "250820", "431960"];
const STEAM_TOOL_NAMES: &[&str] = &[
    "redistributable",
    "proton",
    "steam linux runtime",
    "steamvr",
    "wallpaper engine",
    "steamworks",
];

/// `(name, installdir)` of an `appmanifest_*.acf`, `None` for a tool.
pub fn parse_acf(text: &str) -> Option<(String, String)> {
    let appid = vdf_value(text, "appid")?;
    let name = vdf_value(text, "name")?;
    let dir = vdf_value(text, "installdir")?;
    let lower = name.to_lowercase();
    if STEAM_TOOL_APPIDS.contains(&appid.as_str())
        || STEAM_TOOL_NAMES.iter().any(|t| lower.contains(t))
        || dir.is_empty()
    {
        return None;
    }
    Some((name, dir))
}

/// The value of `key: "value"` in a flat YAML file.
pub fn yaml_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(key)?.strip_prefix(':')?;
        let value = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// An Epic manifest: `(name, install folder, launched program's full path)`.
pub fn parse_epic(text: &str) -> Option<(String, String, Option<String>)> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    if json.get("bIsIncompleteInstall").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    if let Some(categories) = json.get("AppCategories").and_then(|v| v.as_array())
        && !categories.iter().any(|c| c.as_str() == Some("games"))
    {
        return None;
    }
    let root = json.get("InstallLocation")?.as_str()?.to_string();
    let name = json.get("DisplayName").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let exe = json
        .get("LaunchExecutable")
        .and_then(|v| v.as_str())
        .filter(|e| !e.is_empty())
        .map(|e| format!("{}\\{}", root.trim_end_matches(['\\', '/']), e));
    Some((name, root, exe))
}

/// `valorant.live` -> the name and, for the games whose real program is
/// known, its path inside the install folder.
fn riot_product(id: &str) -> (String, Option<&'static str>) {
    let (product, patchline) = id.split_once('.').unwrap_or((id, "live"));
    let (name, exe) = match product {
        "valorant" => (
            "VALORANT".to_string(),
            Some("ShooterGame\\Binaries\\Win64\\VALORANT-Win64-Shipping.exe"),
        ),
        "league_of_legends" => {
            ("League of Legends".to_string(), Some("Game\\League of Legends.exe"))
        }
        "teamfighttactics" => ("Teamfight Tactics".to_string(), None),
        "bacon" => ("Legends of Runeterra".to_string(), None),
        "lion" => ("2XKO".to_string(), None),
        other => (crate::games::name_from_exe(other), None),
    };
    if patchline == "live" {
        (name, exe)
    } else {
        (format!("{name} ({})", patchline.to_uppercase()), exe)
    }
}

// ----------------------------------------------------------------- registry

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(root: HKEY, path: &str) -> Option<Key> {
    let mut key = HKEY::default();
    // SAFETY: `key` outlives the call.
    let status = unsafe { RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &mut key) };
    (status == ERROR_SUCCESS).then_some(Key(key))
}

fn subkeys(root: HKEY, path: &str) -> Vec<String> {
    let Some(key) = open(root, path) else { return Vec::new() };
    let mut out = Vec::new();
    for index in 0..4096u32 {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        // SAFETY: `name` and `len` outlive the call; `len` is its capacity.
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    out
}

fn read_str(root: HKEY, path: &str, value: &str) -> Option<String> {
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` holds `size` bytes and outlives the call.
    let status = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(path),
            &HSTRING::from(value),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

// ------------------------------------------------------------------ sources

/// Everything installed, from every source.
pub fn scan() -> Installed {
    let mut found = Installed::default();
    from_game_config_store(&mut found);
    from_steam(&mut found);
    from_epic(&mut found);
    from_riot(&mut found);
    from_gog(&mut found);
    from_ubisoft(&mut found);
    from_ea(&mut found);
    from_battle_net(&mut found);
    from_xbox(&mut found);
    infer_main_exes(&mut found);
    found
}

fn from_game_config_store(found: &mut Installed) {
    const CHILDREN: &str = "System\\GameConfigStore\\Children";
    for child in subkeys(HKEY_CURRENT_USER, CHILDREN) {
        if let Some(path) =
            read_str(HKEY_CURRENT_USER, &format!("{CHILDREN}\\{child}"), "MatchedExeFullPath")
        {
            found.add_exe(&path, "");
        }
    }
}

fn steam_path() -> Option<PathBuf> {
    let path = PathBuf::from(read_str(HKEY_CURRENT_USER, "Software\\Valve\\Steam", "SteamPath")?);
    path.is_dir().then_some(path)
}

fn from_steam(found: &mut Installed) {
    let Some(steam) = steam_path() else {
        tracing::debug!("no Steam install");
        return;
    };
    let mut libraries = vec![steam.clone()];
    match std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        Ok(vdf) => libraries.extend(crate::autoclip::library_paths(&vdf)),
        Err(e) => tracing::debug!(error = %e, "Steam libraryfolders.vdf unreadable"),
    }
    let mut seen: Vec<String> = Vec::new();
    for library in libraries {
        let key = norm(&library.to_string_lossy());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let apps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&apps) else { continue };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_lowercase();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            if let Some((name, dir)) = parse_acf(&text) {
                let root = apps.join("common").join(dir);
                if root.is_dir() {
                    found.add_root(&root.to_string_lossy(), &name);
                }
            }
        }
    }
}

fn program_data() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
}

fn from_epic(found: &mut Installed) {
    let dir = program_data().join("Epic\\EpicGamesLauncher\\Data\\Manifests");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        tracing::debug!("no Epic manifests");
        return;
    };
    for entry in entries.flatten() {
        if entry.path().extension().is_none_or(|e| !e.eq_ignore_ascii_case("item")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        match parse_epic(&text) {
            Some((name, root, exe)) => {
                if !Path::new(&root).is_dir() {
                    continue;
                }
                found.add_root(&root, &name);
                if let Some(exe) = exe {
                    found.add_exe(&exe, &name);
                }
            }
            None => tracing::debug!(file = %entry.path().display(), "Epic manifest skipped"),
        }
    }
}

fn from_riot(found: &mut Installed) {
    let dir = program_data().join("Riot Games\\Metadata");
    let Ok(products) = std::fs::read_dir(&dir) else {
        tracing::debug!("no Riot metadata");
        return;
    };
    for product in products.flatten() {
        let id = product.file_name().to_string_lossy().to_string();
        let settings = product.path().join(format!("{id}.product_settings.yaml"));
        let Ok(text) = std::fs::read_to_string(&settings) else { continue };
        let Some(root) = yaml_value(&text, "product_install_full_path") else { continue };
        if !Path::new(&root).is_dir() {
            continue;
        }
        let (name, exe) = riot_product(&id);
        found.add_root(&root, &name);
        if let Some(exe) = exe {
            found.add_exe(&format!("{root}\\{exe}"), &name);
        }
    }
}

fn from_gog(found: &mut Installed) {
    const GAMES: &str = "SOFTWARE\\WOW6432Node\\GOG.com\\Games";
    for id in subkeys(HKEY_LOCAL_MACHINE, GAMES) {
        let key = format!("{GAMES}\\{id}");
        let name = read_str(HKEY_LOCAL_MACHINE, &key, "gameName").unwrap_or_default();
        let Some(root) = read_str(HKEY_LOCAL_MACHINE, &key, "path") else { continue };
        found.add_root(&root, &name);
        if let Some(exe) = read_str(HKEY_LOCAL_MACHINE, &key, "exe") {
            let exe = if exe.contains(':') { exe } else { format!("{root}\\{exe}") };
            found.add_exe(&exe, &name);
        }
    }
}

fn last_component(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

fn from_ubisoft(found: &mut Installed) {
    const INSTALLS: &str = "SOFTWARE\\WOW6432Node\\Ubisoft\\Launcher\\Installs";
    for id in subkeys(HKEY_LOCAL_MACHINE, INSTALLS) {
        if let Some(root) = read_str(HKEY_LOCAL_MACHINE, &format!("{INSTALLS}\\{id}"), "InstallDir")
            && Path::new(&root).is_dir()
        {
            found.add_root(&root, &last_component(&root));
        }
    }
}

fn from_ea(found: &mut Installed) {
    const GAMES: &str = "SOFTWARE\\WOW6432Node\\EA Games";
    for name in subkeys(HKEY_LOCAL_MACHINE, GAMES) {
        if let Some(root) = read_str(HKEY_LOCAL_MACHINE, &format!("{GAMES}\\{name}"), "Install Dir")
            && Path::new(&root).is_dir()
        {
            found.add_root(&root, &name);
        }
    }
}

fn from_battle_net(found: &mut Installed) {
    const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    const UNINSTALL32: &str =
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    for (root, path) in [
        (HKEY_LOCAL_MACHINE, UNINSTALL32),
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (HKEY_CURRENT_USER, UNINSTALL),
    ] {
        for id in subkeys(root, path) {
            let key = format!("{path}\\{id}");
            if read_str(root, &key, "Publisher").as_deref() != Some("Blizzard Entertainment") {
                continue;
            }
            let name = read_str(root, &key, "DisplayName").unwrap_or_default();
            if name.eq_ignore_ascii_case("battle.net") {
                continue;
            }
            if let Some(dir) = read_str(root, &key, "InstallLocation")
                && Path::new(&dir).is_dir()
            {
                found.add_root(&dir, &name);
            }
        }
    }
}

fn from_xbox(found: &mut Installed) {
    let Ok(entries) = std::fs::read_dir("C:\\XboxGames") else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if ["gamesave", "gamesaves", "temp", ".gamingroot"].contains(&name.to_lowercase().as_str())
            || !entry.path().is_dir()
        {
            continue;
        }
        found.add_root(&entry.path().to_string_lossy(), &name);
    }
}

/// Folder names that hold helpers, not the game.
const HELPER_DIRS: &[&str] = &[
    "redist",
    "easyanticheat",
    "battleye",
    "directx",
    "dotnet",
    "vcredist",
    "__installer",
    "installers",
    "support",
    "tools",
];
/// Program names that are helpers, not the game.
const HELPER_EXES: &[&str] = &[
    "eac",
    "anticheat",
    "battleye",
    "_be.exe",
    "helper",
    "server",
    "uninst",
    "redist",
    "dxsetup",
    "benchmark",
    "config",
    "editor",
    "report",
    "vc_",
];

/// For an install folder with no known program, the game's program when it is
/// unambiguous: the one Unreal `*-Shipping.exe`, else the only program left
/// once helpers are set aside.
pub fn pick_main_exe(candidates: &[String]) -> Option<String> {
    let kept: Vec<&String> = candidates
        .iter()
        .filter(|path| {
            let exe = path.rsplit('\\').next().unwrap_or(path);
            !crate::game::excluded(path, exe) && !HELPER_EXES.iter().any(|h| exe.contains(h))
        })
        .collect();
    let shipping: Vec<&&String> = kept.iter().filter(|p| p.ends_with("-shipping.exe")).collect();
    match (shipping.as_slice(), kept.as_slice()) {
        ([one], _) => Some((**one).clone()),
        ([], [one]) => Some((*one).clone()),
        _ => None,
    }
}

/// Programs under `root`, a few folders deep, helper folders skipped.
fn exes_under(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    let mut visited = 0u32;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > 2000 || out.len() > 64 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if kind.is_dir() {
                if depth < 5 && !HELPER_DIRS.iter().any(|h| name.contains(h)) {
                    stack.push((entry.path(), depth + 1));
                }
            } else if name.ends_with(".exe") {
                out.push(norm(&entry.path().to_string_lossy()));
            }
        }
    }
    out
}

/// Gives each install folder that has no listable program its main program,
/// when it can be told.
/// Each folder's main program, once found (or found missing), so the scan
/// every ten minutes does not walk every game folder again.
static MAIN_EXES: std::sync::Mutex<Vec<(String, Option<String>)>> = std::sync::Mutex::new(Vec::new());

fn infer_main_exes(found: &mut Installed) {
    let listed = found.games();
    let roots = found.roots.clone();
    for (root, name) in roots {
        let prefix = format!("{root}\\");
        if listed.iter().any(|(path, _)| path.starts_with(&prefix)) {
            continue;
        }
        let cached = MAIN_EXES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|(r, _)| *r == root)
            .map(|(_, exe)| exe.clone());
        let exe = match cached {
            Some(exe) => exe,
            None => {
                let exe = pick_main_exe(&exes_under(Path::new(&root)));
                MAIN_EXES
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((root.clone(), exe.clone()));
                exe
            }
        };
        if let Some(exe) = exe {
            found.add_exe(&exe, &name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACF: &str = r#""AppState"
{
	"appid"		"730"
	"universe"		"1"
	"name"		"Counter-Strike 2"
	"StateFlags"		"4"
	"installdir"		"Counter-Strike Global Offensive"
}"#;

    #[test]
    fn an_app_manifest_gives_the_name_and_folder_and_tools_are_skipped() {
        assert_eq!(
            parse_acf(ACF),
            Some(("Counter-Strike 2".into(), "Counter-Strike Global Offensive".into()))
        );
        let redist = ACF
            .replace("\"730\"", "\"228980\"")
            .replace("Counter-Strike 2", "Steamworks Common Redistributables");
        assert_eq!(parse_acf(&redist), None);
        let proton = ACF.replace("\"730\"", "\"1\"").replace("Counter-Strike 2", "Proton 9.0");
        assert_eq!(parse_acf(&proton), None);
        assert_eq!(parse_acf("garbage"), None);
    }

    #[test]
    fn a_yaml_line_is_read_without_its_quotes() {
        let text = "product_install_root: \"C:/Riot Games\"\nproduct_install_full_path: \"C:/Riot Games/VALORANT/live\"\n";
        assert_eq!(
            yaml_value(text, "product_install_full_path").as_deref(),
            Some("C:/Riot Games/VALORANT/live")
        );
        assert_eq!(yaml_value(text, "product_install_root").as_deref(), Some("C:/Riot Games"));
        assert_eq!(yaml_value(text, "missing"), None);
    }

    #[test]
    fn an_epic_manifest_gives_the_folder_and_program_and_non_games_are_skipped() {
        let item = r#"{"DisplayName":"Rocket League","InstallLocation":"C:\\Program Files\\Epic Games\\rocketleague","LaunchExecutable":"Binaries/Win64/RocketLeague.exe","AppCategories":["public","games","applications"]}"#;
        let (name, root, exe) = parse_epic(item).unwrap();
        assert_eq!(name, "Rocket League");
        assert_eq!(root, "C:\\Program Files\\Epic Games\\rocketleague");
        assert_eq!(
            exe.as_deref(),
            Some("C:\\Program Files\\Epic Games\\rocketleague\\Binaries/Win64/RocketLeague.exe")
        );
        let engine = item.replace("\"games\",", "");
        assert_eq!(parse_epic(&engine), None);
        assert_eq!(parse_epic("{"), None);
    }

    #[test]
    fn riot_products_map_to_their_real_program() {
        assert_eq!(
            riot_product("valorant.live").1,
            Some("ShooterGame\\Binaries\\Win64\\VALORANT-Win64-Shipping.exe")
        );
        assert_eq!(riot_product("league_of_legends.pbe").0, "League of Legends (PBE)");
    }

    #[test]
    fn classify_knows_exact_programs_and_folders_but_not_neighbours() {
        let mut i = Installed::default();
        i.add_exe(
            "C:/Riot Games/VALORANT/live/ShooterGame/Binaries/Win64/VALORANT-Win64-Shipping.exe",
            "VALORANT",
        );
        i.add_root("C:\\Steam\\steamapps\\common\\Aim Lab\\", "Aim Lab");
        i.add_root("C:\\", "bad");
        i.add_root("C:\\Epic\\rocketleague", "Rocket League®");
        i.add_exe("C:\\Epic\\rocketleague\\Binaries\\Win64\\RocketLeague.exe", "");
        assert_eq!(
            i.classify("C:\\Epic\\rocketleague\\Binaries\\Win64\\RocketLeague.exe"),
            Some(("Rocket League".into(), true)),
            "an unnamed program takes its folder's name"
        );
        assert_eq!(
            i.classify("c:\\riot games\\valorant\\live\\shootergame\\binaries\\win64\\valorant-win64-shipping.exe"),
            Some(("VALORANT".into(), true))
        );
        assert_eq!(
            i.classify("C:\\Steam\\steamapps\\common\\Aim Lab\\AimLab_tb.exe"),
            Some(("Aim Lab".into(), false))
        );
        assert_eq!(i.classify("C:\\Steam\\steamapps\\common\\Aim Lab2\\x.exe"), None);
        assert_eq!(
            i.classify("C:\\Windows\\notepad.exe"),
            None,
            "a drive root is never an install folder"
        );
    }

    #[test]
    fn the_main_program_is_picked_only_when_unambiguous() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick_main_exe(&s(&[
                "c:\\g\\launcher.exe",
                "c:\\g\\rocketleague.exe",
                "c:\\g\\rocketleague_eac.exe"
            ]))
            .as_deref(),
            Some("c:\\g\\rocketleague.exe")
        );
        assert_eq!(
            pick_main_exe(&s(&[
                "c:\\g\\x.exe",
                "c:\\g\\bin\\x-win64-shipping.exe",
                "c:\\g\\crashreportclient.exe"
            ]))
            .as_deref(),
            Some("c:\\g\\bin\\x-win64-shipping.exe")
        );
        assert_eq!(pick_main_exe(&s(&["c:\\g\\a.exe", "c:\\g\\b.exe"])), None);
    }

    /// What this PC has installed. `cargo test -p clippy-daemon installed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_what_this_pc_has() {
        let found = scan();
        for (root, name) in &found.roots {
            println!("folder  {name:30} {root}");
        }
        for (path, name) in &found.exes {
            println!("program {name:30} {path}");
        }
        for (path, name) in found.games() {
            println!("GAME    {name:30} {path}");
        }
    }
}
