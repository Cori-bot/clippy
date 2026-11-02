//! Telling whether a game is in front, for the `game` capture mode.
//!
//! NVIDIA's overlay records the game and leaves the desktop alone unless the
//! user asks for it. Clippy does the same with what Windows lets any program
//! see: the window in front, whether it covers its whole screen, and which
//! program owns it. A program installed as a game (see [`crate::installed`])
//! or added by the user is a game, windowed or not, and goes in the games
//! list. A full-screen window of any other program that is not a browser, a
//! player, a launcher or part of Windows is only a guess: the `game` capture
//! mode records it, but it never joins the list.

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW,
            GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            WS_EX_TOOLWINDOW,
        },
    },
    core::{HSTRING, PWSTR},
};

/// Programs that are often full screen without being games: the shell, web
/// browsers (a full-screen video), media players, meeting and remote-desktop
/// tools, presentations. A user who plays something from this list adds it to
/// `game_apps`, which wins.
const NOT_GAMES: &[&str] = &[
    "explorer.exe",
    "searchhost.exe",
    "applicationframehost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "lockapp.exe",
    "taskmgr.exe",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "arc.exe",
    "vlc.exe",
    "mpv.exe",
    "wmplayer.exe",
    "potplayermini64.exe",
    "spotify.exe",
    "discord.exe",
    "teams.exe",
    "ms-teams.exe",
    "zoom.exe",
    "mstsc.exe",
    "powerpnt.exe",
    "code.exe",
    "obs64.exe",
    "clippy-ui.exe",
    "clippy-daemon.exe",
    // Screen capture overlays (Win+Shift+S covers every screen).
    "snippingtool.exe",
    "screenclippinghost.exe",
    "screensketch.exe",
    "sharex.exe",
    "greenshot.exe",
    "lightshot.exe",
    // Shell and system pieces.
    "textinputhost.exe",
    "systemsettings.exe",
    "searchapp.exe",
    "widgets.exe",
    "gamebar.exe",
    "gamebarftserver.exe",
    "nvcontainer.exe",
    "nvidia overlay.exe",
    "nvidia app.exe",
    "nvidia share.exe",
    "msedgewebview2.exe",
    // Store clients and game launchers.
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "riotclientux.exe",
    "riotclientuxrender.exe",
    "riotclientservices.exe",
    "leagueclient.exe",
    "leagueclientux.exe",
    "leagueclientuxrender.exe",
    "battle.net.exe",
    "eadesktop.exe",
    "upc.exe",
    "ubisoftconnect.exe",
    "galaxyclient.exe",
    // Players and viewers.
    "video.ui.exe",
    "microsoft.media.player.exe",
    "mpc-hc64.exe",
    "mpc-be64.exe",
    "photos.exe",
];

/// Parts of program names that are never the game: crash reporters,
/// installers, updaters and launchers.
const NOT_GAME_PARTS: &[&str] =
    &["crashreport", "crashhandler", "unins", "setup", "updater", "launcher"];

/// Whether this program is never a game by itself: on the list above, named
/// like a helper, or part of Windows. `path` may be the bare file name when the
/// full path is not known.
pub fn excluded(path: &str, exe: &str) -> bool {
    let exe = exe.to_lowercase();
    let path = path.to_lowercase().replace('/', "\\");
    NOT_GAMES.contains(&exe.as_str())
        || NOT_GAME_PARTS.iter().any(|part| exe.contains(part))
        || path.starts_with(&windows_dir())
}

/// `c:\windows\` on most machines, wherever Windows actually lives.
fn windows_dir() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    format!("{}\\", root.trim_end_matches('\\').to_lowercase())
}

/// Window classes of the desktop itself: its wallpaper host and the taskbar.
const SHELL_CLASSES: &[&str] = &["progman", "workerw", "shell_traywnd", "shell_secondarytraywnd"];

/// A game that is in front or running: its program, the name to show for it,
/// and where its screen starts (to find which of Clippy's numbered screens it
/// is on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub exe: String,
    /// Product name from the program's version resource, else its file name.
    pub name: String,
    pub left: i32,
    pub top: i32,
    /// An installed game or one the user added, as opposed to a full-screen
    /// program Clippy only guesses is a game. Only known games go in the
    /// games list.
    pub known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for Bounds {
    fn from(r: RECT) -> Self {
        Self { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

/// Whether a window fills its screen. A pixel of slack on each side: borderless
/// windows are often created one pixel off, and exclusive full screen reports
/// the screen exactly.
pub fn covers(window: Bounds, screen: Bounds) -> bool {
    window.left <= screen.left + 1
        && window.top <= screen.top + 1
        && window.right >= screen.right - 1
        && window.bottom >= screen.bottom - 1
}

/// What the user told Clippy about programs, as the detector needs it.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    /// Games even windowed.
    pub games: Vec<String>,
    /// Never games, even full screen.
    pub never: Vec<String>,
}

/// What a program in front is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    No,
    /// Added by the user or installed as a game, windowed or not. Carries the
    /// installed game's name when the scan knows it.
    Known(Option<String>),
    /// Full screen and not known: recorded in the `game` capture mode, never
    /// added to the games list.
    Guess,
}

/// Whether this program counts as a game right now.
///
/// The user's lists win over everything: a program they switched off is never
/// a game, one they added is a game windowed or not, because that is the one
/// signal that cannot be wrong. Then helpers and Windows itself are never
/// games, an installed game is one even windowed, and anything else full
/// screen is only a guess.
pub fn verdict(
    path: &str,
    exe: &str,
    lists: &Lists,
    fills_screen: bool,
    installed: &crate::installed::Installed,
) -> Verdict {
    if lists.never.iter().any(|g| g == exe) {
        return Verdict::No;
    }
    if lists.games.iter().any(|g| g == exe) {
        return Verdict::Known(None);
    }
    if excluded(path, exe) {
        return Verdict::No;
    }
    if let Some((name, _)) = installed.classify(path) {
        return Verdict::Known((!name.is_empty()).then_some(name));
    }
    if fills_screen { Verdict::Guess } else { Verdict::No }
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// File name, lower-cased, of the program with this process id.
#[cfg(test)]
fn exe_of(pid: u32) -> Option<String> {
    image_path_of(pid).and_then(|path| file_name_lower(&path))
}

fn file_name_lower(path: &str) -> Option<String> {
    path.rsplit(['\\', '/']).next().map(str::to_lowercase)
}

/// Full path of the program with this process id.
fn image_path_of(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed before returning; the buffer outlives the call.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(wide_to_string(&buffer[..len as usize]))
    }
}

/// What to call a program: the product name in its version resource (what
/// Discord shows too), else its description, else the file name.
pub fn friendly_name(path: &str, exe: &str) -> String {
    read_version_string(path).unwrap_or_else(|| crate::games::name_from_exe(exe))
}

fn read_version_string(path: &str) -> Option<String> {
    let wide = HSTRING::from(path);
    // SAFETY: `info` is sized by the call just before it; every pointer
    // `VerQueryValueW` returns points into `info` and is read before `info`
    // is dropped.
    unsafe {
        let size = GetFileVersionInfoSizeW(&wide, None);
        if size == 0 {
            return None;
        }
        let mut info = vec![0u8; size as usize];
        GetFileVersionInfoW(&wide, None, size, info.as_mut_ptr().cast()).ok()?;
        let mut translation: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let language = if VerQueryValueW(
            info.as_ptr().cast(),
            &HSTRING::from("\\VarFileInfo\\Translation"),
            &mut translation,
            &mut len,
        )
        .as_bool()
            && len >= 4
        {
            let pair = std::slice::from_raw_parts(translation.cast::<u16>(), 2);
            format!("{:04x}{:04x}", pair[0], pair[1])
        } else {
            "040904b0".to_string()
        };
        for key in ["ProductName", "FileDescription"] {
            let query = HSTRING::from(format!("\\StringFileInfo\\{language}\\{key}"));
            let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut chars = 0u32;
            if VerQueryValueW(info.as_ptr().cast(), &query, &mut value, &mut chars).as_bool()
                && chars > 1
            {
                let text =
                    wide_to_string(std::slice::from_raw_parts(value.cast::<u16>(), chars as usize));
                // Trademark signs are part of the resource, not of the name a
                // person says: "Microsoft® Windows®" reads as "Microsoft Windows".
                let text: String = text.chars().filter(|c| !matches!(c, '®' | '™')).collect();
                let text = text.trim();
                if !text.is_empty() && text.chars().count() <= 48 {
                    return Some(text.to_string());
                }
            }
        }
        None
    }
}

/// A window open on this PC, for the "add a game" picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWindow {
    pub exe: String,
    pub name: String,
    pub title: String,
}

/// The programs with a window the user can see, one entry per program, the way
/// Discord's picker lists them.
pub fn list_windows() -> Vec<OpenWindow> {
    struct Found {
        windows: Vec<(u32, String)>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        // SAFETY: `lparam` is the `&mut Found` passed to `EnumWindows` below,
        // alive for the whole enumeration.
        let found = unsafe { &mut *(lparam.0 as *mut Found) };
        // SAFETY: plain queries on a window handle EnumWindows just gave us;
        // every out-parameter is a local that lives through its call.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return true.into();
            }
            if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return true.into();
            }
            // Windows keeps windows of other virtual desktops and suspended
            // UWP apps "visible" but cloaked.
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                size_of::<u32>() as u32,
            )
            .is_ok()
                && cloaked != 0
            {
                return true.into();
            }
            let mut title = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            let title = wide_to_string(&title[..len]);
            if title.trim().is_empty() {
                return true.into();
            }
            let mut class = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            if SHELL_CLASSES.contains(&wide_to_string(&class[..len]).to_lowercase().as_str()) {
                return true.into();
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != 0 && pid != std::process::id() {
                found.windows.push((pid, title));
            }
        }
        true.into()
    }

    let mut found = Found { windows: Vec::new() };
    // SAFETY: the callback only runs inside this call, with `found` alive.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut Found as isize)) };

    let mut out: Vec<OpenWindow> = Vec::new();
    for (pid, title) in found.windows {
        let Some(path) = image_path_of(pid) else { continue };
        let Some(exe) = file_name_lower(&path) else { continue };
        if crate::games::normalize_exe(&exe).is_none()
            || [
                "clippy-ui.exe",
                "clippy-daemon.exe",
                "applicationframehost.exe",
                "textinputhost.exe",
            ]
            .contains(&exe.as_str())
            || out.iter().any(|w| w.exe == exe)
        {
            continue;
        }
        let name = friendly_name(&path, &exe);
        out.push(OpenWindow { exe, name, title });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn is_alive(pid: u32) -> bool {
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: the handle is closed before returning.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(process, &mut code).is_ok() && code == STILL_ACTIVE;
        let _ = CloseHandle(process);
        alive
    }
}

/// The window in front, if it is a game: `(pid, sighting)`.
fn foreground_game(lists: &Lists) -> Option<(u32, Sighting)> {
    // SAFETY: plain queries on the window in front; every out-parameter is a
    // local that lives through its call.
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        if SHELL_CLASSES.contains(&wide_to_string(&class[..len]).to_lowercase().as_str()) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let path = image_path_of(pid)?;
        let exe = file_name_lower(&path)?;

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info =
            MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let screen = Bounds::from(info.rcMonitor);
        let fills = covers(Bounds::from(rect), screen);
        let (known, name) = match verdict(&path, &exe, lists, fills, &crate::installed::current()) {
            Verdict::No => return None,
            Verdict::Known(name) => (true, name),
            Verdict::Guess => (false, None),
        };
        let name = name.unwrap_or_else(|| friendly_name(&path, &exe));
        Some((pid, Sighting { exe, name, left: screen.left, top: screen.top, known }))
    }
}

/// Follows the game across alt-tabs: a game that is no longer in front is still
/// the game for as long as its process runs, so reading a browser for a minute
/// does not end the replay and restart it with the ring empty.
#[derive(Default)]
pub struct Tracker {
    tracked: Option<(u32, Sighting)>,
}

impl Tracker {
    pub fn poll(&mut self, lists: &Lists) -> Option<Sighting> {
        if let Some(found) = foreground_game(lists) {
            self.tracked = Some(found);
        } else if let Some((pid, _)) = &self.tracked
            && !is_alive(*pid)
        {
            self.tracked = None;
        }
        self.tracked.as_ref().map(|(_, sighting)| sighting.clone())
    }
}

/// Starts the thread that looks for a game once a second.
///
/// It runs in every capture mode: the games list fills up as the user plays,
/// whichever mode they are in. Only the `game` mode acts on what it finds, by
/// starting and stopping the replay. The look itself is a few window queries.
pub fn spawn_watch(daemon: std::sync::Arc<crate::state::Daemon>) {
    use clippy_core::config::CaptureMode;
    let daemon_scan = std::sync::Arc::clone(&daemon);
    let spawned = std::thread::Builder::new().name("clippy-game-watch".into()).spawn(move || {
        let mut tracker = Tracker::default();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let lists = daemon.game_lists();
            let seen = tracker.poll(&lists);
            if let Some(found) = &seen {
                daemon.game_seen(found);
            }
            if daemon.capture_mode() == CaptureMode::Game {
                daemon.game_tick(seen, std::time::Instant::now());
            } else {
                daemon.note_sighting(seen, std::time::Instant::now());
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the game watcher; the game capture mode will not start by itself");
    }
    // The installed games: at startup, then every ten minutes for games
    // installed while Clippy runs. Each scan adds the new ones to the list.
    let spawned =
        std::thread::Builder::new().name("clippy-installed-games".into()).spawn(move || {
            loop {
                crate::installed::rescan();
                daemon_scan.games_add_installed();
                std::thread::sleep(std::time::Duration::from_secs(600));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the installed-games scan");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Bounds = Bounds { left: 0, top: 0, right: 1920, bottom: 1080 };

    fn b(left: i32, top: i32, right: i32, bottom: i32) -> Bounds {
        Bounds { left, top, right, bottom }
    }

    #[test]
    fn a_window_covering_the_screen_fills_it_with_a_pixel_of_slack() {
        assert!(covers(SCREEN, SCREEN));
        assert!(covers(b(-1, -1, 1921, 1081), SCREEN), "exclusive full screen overshoots");
        assert!(covers(b(1, 1, 1919, 1079), SCREEN), "a borderless window one pixel in");
    }

    #[test]
    fn a_maximised_window_with_a_taskbar_does_not_fill_the_screen() {
        assert!(!covers(b(0, 0, 1920, 1040), SCREEN));
        assert!(!covers(b(100, 100, 1500, 900), SCREEN));
    }

    #[test]
    fn a_second_screen_is_measured_against_itself() {
        let second = b(1920, 0, 3840, 1080);
        assert!(covers(second, second));
        assert!(!covers(SCREEN, second));
    }

    use crate::installed::Installed;

    fn v(path: &str, lists: &Lists, fills: bool, installed: &Installed) -> Verdict {
        let exe = file_name_lower(path).unwrap();
        verdict(path, &exe, lists, fills, installed)
    }

    #[test]
    fn an_unknown_full_screen_program_is_only_a_guess() {
        let none = Lists::default();
        let nothing = Installed::default();
        assert_eq!(v("D:\\Games\\eldenring.exe", &none, true, &nothing), Verdict::Guess);
        assert_eq!(
            v("D:\\Games\\eldenring.exe", &none, false, &nothing),
            Verdict::No,
            "windowed and unknown"
        );
        assert_eq!(
            v("C:\\b\\brave.exe", &none, true, &nothing),
            Verdict::No,
            "a full-screen video is not a game"
        );
        assert_eq!(v("C:\\Windows\\explorer.exe", &none, true, &nothing), Verdict::No);
    }

    #[test]
    fn capture_overlays_launchers_and_windows_are_never_games() {
        let none = Lists::default();
        let mut installed = Installed::default();
        installed.add_root("C:\\Riot Games\\League of Legends", "League of Legends");
        for path in [
            "C:\\Windows\\System32\\SnippingTool.exe",
            "C:\\Program Files\\WindowsApps\\Microsoft.ScreenSketch_11\\SnippingTool\\SnippingTool.exe",
            "C:\\Program Files\\WindowsApps\\Microsoft.Windows.Photos_1\\Photos.exe",
            "C:\\Riot Games\\League of Legends\\LeagueClientUx.exe",
            "C:\\Games\\X\\CrashReportClient.exe",
            "C:\\Games\\X\\unins000.exe",
            "C:\\Games\\X\\Launcher.exe",
            "C:\\Program Files (x86)\\Steam\\steam.exe",
        ] {
            assert_eq!(v(path, &none, true, &installed), Verdict::No, "{path}");
        }
    }

    #[test]
    fn an_installed_game_is_known_even_windowed() {
        let none = Lists::default();
        let mut installed = Installed::default();
        installed.add_root("C:\\Steam\\steamapps\\common\\Aim Lab", "Aim Lab");
        installed.add_exe(
            "C:\\Riot Games\\League of Legends\\Game\\League of Legends.exe",
            "League of Legends",
        );
        assert_eq!(
            v("C:\\Steam\\steamapps\\common\\Aim Lab\\AimLab_tb.exe", &none, false, &installed),
            Verdict::Known(Some("Aim Lab".into()))
        );
        assert_eq!(
            v(
                "C:\\Riot Games\\League of Legends\\Game\\League of Legends.exe",
                &none,
                true,
                &installed
            ),
            Verdict::Known(Some("League of Legends".into()))
        );
    }

    #[test]
    fn the_users_list_wins_in_both_directions_it_can() {
        let mine = Lists {
            games: vec!["brave.exe".to_string(), "minecraft.exe".to_string()],
            never: vec!["eldenring.exe".to_string()],
        };
        let mut installed = Installed::default();
        installed.add_exe("D:\\Games\\eldenring.exe", "Elden Ring");
        assert_eq!(
            v("C:\\m\\minecraft.exe", &mine, false, &installed),
            Verdict::Known(None),
            "windowed but listed"
        );
        assert_eq!(
            v("C:\\b\\brave.exe", &mine, true, &installed),
            Verdict::Known(None),
            "listed beats the built-in non-games"
        );
        assert_eq!(
            v("D:\\Games\\eldenring.exe", &mine, true, &installed),
            Verdict::No,
            "switched off beats installed"
        );
    }

    /// Against the real machine: whatever is in front, asking must not fail,
    /// and a process that does not exist is not alive.
    #[test]
    fn the_real_queries_run() {
        let mut tracker = Tracker::default();
        let _ = tracker.poll(&Lists::default());
        assert!(is_alive(std::process::id()));
        assert!(!is_alive(u32::MAX - 1));
        assert!(exe_of(std::process::id()).is_some());
    }

    #[test]
    fn a_program_is_named_from_its_resource_or_its_file() {
        let me = image_path_of(std::process::id()).expect("own path");
        let exe = exe_of(std::process::id()).expect("own exe");
        assert!(!friendly_name(&me, &exe).is_empty());
        assert_eq!(friendly_name("C:\\nope\\eldenring.exe", "eldenring.exe"), "Eldenring");
    }

    #[test]
    fn listing_windows_runs_and_never_lists_clippy() {
        for window in list_windows() {
            assert!(crate::games::normalize_exe(&window.exe).is_some());
            assert!(!window.title.trim().is_empty());
            assert_ne!(window.exe, "clippy-daemon.exe");
        }
    }
}
