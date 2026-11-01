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
];

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

/// Whether this program counts as a game right now.
///
/// The user's lists win over everything: a program they switched off is never
/// a game, one they added is a game windowed or not, because that is the one
/// signal that cannot be wrong. Otherwise it takes a window that fills its
/// screen from a program that is not on the list above.
pub fn is_game(exe: &str, lists: &Lists, fills_screen: bool) -> bool {
    if lists.never.iter().any(|g| g == exe) {
        return false;
    }
    if lists.games.iter().any(|g| g == exe) {
        return true;
    }
    fills_screen && !NOT_GAMES.contains(&exe)
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
                let text = wide_to_string(std::slice::from_raw_parts(
                    value.cast::<u16>(),
                    chars as usize,
                ));
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
            || ["clippy-ui.exe", "clippy-daemon.exe", "applicationframehost.exe", "textinputhost.exe"]
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
        if !is_game(&exe, lists, fills) {
            return None;
        }
        let name = friendly_name(&path, &exe);
        Some((pid, Sighting { exe, name, left: screen.left, top: screen.top }))
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
        let none = Lists::default();
        assert!(is_game("eldenring.exe", &none, true));
        assert!(!is_game("eldenring.exe", &none, false), "windowed and unlisted");
        assert!(!is_game("brave.exe", &none, true), "a full-screen video is not a game");
        assert!(!is_game("explorer.exe", &none, true));
    }

    #[test]
    fn the_users_list_wins_in_both_directions_it_can() {
        let mine = Lists {
            games: vec!["brave.exe".to_string(), "minecraft.exe".to_string()],
            never: vec!["eldenring.exe".to_string()],
        };
        assert!(is_game("minecraft.exe", &mine, false), "windowed but listed");
        assert!(is_game("brave.exe", &mine, true), "listed beats the built-in non-games");
        assert!(!is_game("eldenring.exe", &mine, true), "switched off beats full screen");
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
