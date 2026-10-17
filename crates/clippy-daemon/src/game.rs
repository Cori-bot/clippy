//! Telling whether a game is in front, for the `game` capture mode.
//!
//! NVIDIA's overlay records the game and leaves the desktop alone unless the
//! user asks for it. Clippy does the same with what Windows lets any program
//! see: the window in front, whether it covers its whole screen, and which
//! program owns it. A full-screen window of a program that is not a browser, a
//! player or part of the shell is a game; the user can add programs that are
//! games even when windowed.

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic,
            IsWindowVisible,
        },
    },
    core::PWSTR,
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
];

/// Window classes of the desktop itself: its wallpaper host and the taskbar.
const SHELL_CLASSES: &[&str] = &["progman", "workerw", "shell_traywnd", "shell_secondarytraywnd"];

/// A game that is in front or running: its program, and where its screen starts
/// (to find which of Clippy's numbered screens it is on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub exe: String,
    pub left: i32,
    pub top: i32,
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

/// Whether this program counts as a game right now.
///
/// `user_games` wins over everything, windowed or not, because it is the one
/// signal that cannot be wrong: the user said so. Otherwise it takes a window
/// that fills its screen from a program that is not on the list above.
pub fn is_game(exe: &str, user_games: &[String], fills_screen: bool) -> bool {
    if user_games.iter().any(|g| g == exe) {
        return true;
    }
    fills_screen && !NOT_GAMES.contains(&exe)
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// File name, lower-cased, of the program with this process id.
fn exe_of(pid: u32) -> Option<String> {
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
        let path = wide_to_string(&buffer[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_lowercase)
    }
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
fn foreground_game(user_games: &[String]) -> Option<(u32, Sighting)> {
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
        let exe = exe_of(pid)?;

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
        is_game(&exe, user_games, fills)
            .then_some((pid, Sighting { exe, left: screen.left, top: screen.top }))
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
    pub fn poll(&mut self, user_games: &[String]) -> Option<Sighting> {
        if let Some(found) = foreground_game(user_games) {
            self.tracked = Some(found);
        } else if let Some((pid, _)) = &self.tracked
            && !is_alive(*pid)
        {
            self.tracked = None;
        }
        self.tracked.as_ref().map(|(_, sighting)| sighting.clone())
    }
}

/// Starts the thread that looks for a game once a second, but only while
/// `capture_mode` is `game`: in `desktop` mode it sleeps and asks Windows
/// nothing, so the default costs nothing.
pub fn spawn_watch(daemon: std::sync::Arc<crate::state::Daemon>) {
    use clippy_core::config::CaptureMode;
    let spawned = std::thread::Builder::new().name("clippy-game-watch".into()).spawn(move || {
        let mut tracker = Tracker::default();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let (mode, games) = {
                let config = daemon.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                (config.capture_mode(), config.game_apps.clone())
            };
            if mode == CaptureMode::Game {
                let seen = tracker.poll(&games);
                daemon.game_tick(seen, std::time::Instant::now());
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the game watcher; the game capture mode will not start by itself");
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

    #[test]
    fn a_full_screen_program_is_a_game_unless_it_is_a_known_non_game() {
        assert!(is_game("eldenring.exe", &[], true));
        assert!(!is_game("eldenring.exe", &[], false), "windowed and unlisted");
        assert!(!is_game("brave.exe", &[], true), "a full-screen video is not a game");
        assert!(!is_game("explorer.exe", &[], true));
    }

    #[test]
    fn the_users_list_wins_in_both_directions_it_can() {
        let mine = vec!["brave.exe".to_string(), "minecraft.exe".to_string()];
        assert!(is_game("minecraft.exe", &mine, false), "windowed but listed");
        assert!(is_game("brave.exe", &mine, true), "listed beats the built-in non-games");
    }

    /// Against the real machine: whatever is in front, asking must not fail,
    /// and a process that does not exist is not alive.
    #[test]
    fn the_real_queries_run() {
        let mut tracker = Tracker::default();
        let _ = tracker.poll(&[]);
        assert!(is_alive(std::process::id()));
        assert!(!is_alive(u32::MAX - 1));
        assert!(exe_of(std::process::id()).is_some());
    }
}
