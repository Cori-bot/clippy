//! The tray icon, its menu, and the armed/idle artwork (spec §7.2).
//!
//! Everything here runs on the window's pump thread. Tray icons belong to the
//! thread that owns the window they post to, and `TrackPopupMenu` runs its own
//! modal loop that only works on a thread with a message queue — so the `Tray`
//! is created, updated, and destroyed by `window.rs`'s pump and never touched
//! from the worker. The worker asks for a state change by posting a message.

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use serde_json::Value;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, HMENU,
    MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTALIGN, TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR};

/// The tray callback message. Anything from `WM_APP` up is ours.
pub const WM_CLIPPY_TRAY: u32 = WM_APP + 1;

/// Menu command ids, returned by `TrackPopupMenu`.
pub const ID_TOGGLE: usize = 1;
pub const ID_OPEN_UI: usize = 2;
pub const ID_OPEN_FOLDER: usize = 3;
pub const ID_QUIT: usize = 4;
pub const ID_CHANGE_FOLDER: usize = 5;
pub const ID_CLIP: usize = 6;
pub const ID_SCREENSHOT: usize = 7;
pub const ID_AUTOSTART: usize = 8;
pub const ID_AUTO_ARM: usize = 9;
pub const ID_CLIP_SOUND: usize = 10;
/// Submenu items: the id is the base plus the index into the choice table, so
/// [`choice_for_id`] can turn a click back into a config change without a
/// lookup of its own.
const ID_RESOLUTION_BASE: usize = 100;
const ID_FPS_BASE: usize = 200;
const ID_REPLAY_BASE: usize = 300;

/// What the quality submenus offer: a label and the config value it writes.
/// Short lists on purpose -- any value is still one `config.set` away in the
/// app; these are the ones worth a right-click.
pub const RESOLUTION_CHOICES: [(&str, &str); 5] = [
    ("Native", "native"),
    ("480p", "480p"),
    ("720p", "720p"),
    ("1080p", "1080p"),
    ("1440p", "1440p"),
];
pub const FPS_CHOICES: [u32; 5] = [30, 45, 60, 90, 120];
pub const REPLAY_CHOICES: [u32; 5] = [10, 15, 30, 60, 120];

/// What the menu needs to label itself. Gathered by the pump from mirrors (the
/// pump never takes a `Daemon` lock), so it is a plain value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuPrefs {
    pub armed: bool,
    pub autostart: bool,
    pub auto_arm: bool,
    pub clip_sound: bool,
    pub resolution: String,
    pub fps: u32,
    pub replay_seconds: u32,
}

/// The config change a submenu item stands for, or `None` for any other id.
pub fn choice_for_id(id: usize) -> Option<(&'static str, Value)> {
    let index = |base: usize, len: usize| id.checked_sub(base).filter(|i| *i < len);
    if let Some(i) = index(ID_RESOLUTION_BASE, RESOLUTION_CHOICES.len()) {
        return Some(("resolution", Value::from(RESOLUTION_CHOICES[i].1)));
    }
    if let Some(i) = index(ID_FPS_BASE, FPS_CHOICES.len()) {
        return Some(("fps", Value::from(FPS_CHOICES[i])));
    }
    if let Some(i) = index(ID_REPLAY_BASE, REPLAY_CHOICES.len()) {
        return Some(("replay_seconds", Value::from(REPLAY_CHOICES[i])));
    }
    None
}

/// Icon edge in pixels. 32 is the large-DPI tray size; Windows downscales to
/// 16 cleanly and asking for a 16 would look soft at 150% scaling.
pub(crate) const ICON_SIDE: usize = 32;

/// A ring (idle) or a filled disc (armed), as premultiplied BGRA.
///
/// Drawn rather than shipped as an `.ico`: two circles need no resource
/// compiler, no asset files, and no build script, and stay correct if the
/// binary is moved. The shape is deliberately the same silhouette in both
/// states so the icon reads as "Clippy" either way — only the fill changes.
pub(crate) fn icon_pixels(armed: bool) -> Vec<u32> {
    let side = ICON_SIDE as f32;
    let center = (side - 1.0) / 2.0;
    let outer = side * 0.44;
    let inner = outer * 0.55;
    let mut pixels = vec![0u32; ICON_SIDE * ICON_SIDE];
    for y in 0..ICON_SIDE {
        for x in 0..ICON_SIDE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let d = (dx * dx + dy * dy).sqrt();
            // One-pixel smoothstep at each edge: an aliased circle at 16 px
            // looks like a defect.
            let edge = |r: f32| (1.0 - (d - r + 0.5)).clamp(0.0, 1.0);
            let coverage = if armed { edge(outer) } else { edge(outer) * (1.0 - edge(inner)) };
            let alpha = (coverage * 255.0) as u32;
            // Premultiplied white; the tray composites over unknown
            // backgrounds and a non-premultiplied icon fringes dark.
            pixels[y * ICON_SIDE + x] = (alpha << 24) | (alpha << 16) | (alpha << 8) | alpha;
        }
    }
    pixels
}

/// A live tray icon. Removing it is not optional: an icon whose process dies
/// without `NIM_DELETE` lingers in the tray until the user happens to hover
/// it, which looks exactly like a crashed app.
pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 2],
    armed: bool,
}

impl Tray {
    /// # Safety
    /// Must be called on the thread that owns `hwnd`, and the `Tray` must be
    /// dropped on that same thread.
    pub unsafe fn add(hwnd: HWND) -> Result<Self> {
        let icons = [unsafe { make_icon(false) }?, unsafe { make_icon(true) }?];
        let tray = Self { hwnd, icons, armed: false };
        // A failed `NIM_ADD` is kept rather than propagated, because its most
        // likely cause is benign and self-correcting: the daemon autostarts at
        // login (§7.3) and can beat `explorer.exe` to the desktop, so there is
        // no taskbar to add an icon to yet. `readd` runs when the shell
        // announces itself. Only a failure to build the icons is fatal.
        if let Err(e) = unsafe { tray.notify(NIM_ADD) } {
            tracing::warn!(
                error = %format!("{e:#}"),
                "the tray icon did not take; retrying when the shell announces itself"
            );
        }
        Ok(tray)
    }

    /// Re-adds the icon after the shell restarts.
    ///
    /// Windows broadcasts `TaskbarCreated` when `explorer.exe` starts, and
    /// every tray icon that existed before is gone. Without this the daemon
    /// keeps running with no icon — invisible, and looking crashed. It covers
    /// two real cases: explorer restarting (common on the low-end machines
    /// this project targets) and the autostart race above.
    pub fn readd(&self) {
        if let Err(e) = unsafe { self.notify(NIM_ADD) } {
            tracing::warn!(error = %format!("{e:#}"), "could not re-add the tray icon");
        } else {
            tracing::debug!("re-added the tray icon after the shell restarted");
        }
    }

    /// Swaps the icon and the tooltip. Cheap enough to call on every state
    /// change; a `NIM_MODIFY` that changes nothing is skipped outright.
    pub fn set_armed(&mut self, armed: bool) {
        if self.armed == armed {
            return;
        }
        self.armed = armed;
        match unsafe { self.notify(NIM_MODIFY) } {
            // The tray icon is the daemon's only output on a machine with no
            // console, so its state changes are logged: "the icon is wrong" is
            // otherwise an unfalsifiable bug report.
            Ok(()) => tracing::debug!(armed, "tray icon updated"),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not update the tray icon"),
        }
    }

    unsafe fn notify(&self, message: NOTIFY_ICON_MESSAGE) -> Result<()> {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_CLIPPY_TRAY,
            hIcon: self.icons[usize::from(self.armed)],
            ..Default::default()
        };
        // The tooltip is the only text the user gets for "is it recording?"
        // without opening the menu, so it names the state outright.
        let tip = if self.armed { "Clippy — armed" } else { "Clippy — idle" };
        for (i, c) in tip.encode_utf16().enumerate().take(data.szTip.len() - 1) {
            data.szTip[i] = c;
        }
        unsafe { Shell_NotifyIconW(message, &data) }
            .ok()
            .map_err(|e| anyhow!("Shell_NotifyIconW failed: {e}"))
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = self.notify(NIM_DELETE);
            for icon in self.icons {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

unsafe fn make_icon(armed: bool) -> Result<HICON> {
    let pixels = icon_pixels(armed);
    // CreateIcon takes separate AND and XOR masks; with a full alpha channel
    // in the colour bits the AND mask is ignored, so it is all-zero.
    let and_mask = [0u8; ICON_SIDE * ICON_SIDE / 8];
    let xor: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
    unsafe {
        CreateIcon(None, ICON_SIDE as i32, ICON_SIDE as i32, 1, 32, and_mask.as_ptr(), xor.as_ptr())
    }
    .context("CreateIcon")
}

fn item_flags(checked: bool) -> MENU_ITEM_FLAGS {
    if checked { MF_STRING | MF_CHECKED } else { MF_STRING }
}

/// Appends a submenu of single-choice items, the current one ticked.
///
/// # Safety
/// `menu` must be a live menu; the submenu is owned by it and goes with it.
unsafe fn append_choices(menu: HMENU, title: &str, base: usize, choices: &[(String, bool)]) {
    unsafe {
        let Ok(sub) = CreatePopupMenu() else { return };
        for (i, (label, current)) in choices.iter().enumerate() {
            let _ =
                AppendMenuW(sub, item_flags(*current), base + i, &HSTRING::from(label.as_str()));
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, &HSTRING::from(title));
    }
}

/// Shows the tray menu at the cursor and returns the chosen command id.
///
/// `SetForegroundWindow` before `TrackPopupMenu` is required, not decorative:
/// without it the menu does not dismiss when the user clicks elsewhere and
/// stays stuck on screen. The `WM_NULL` afterwards is the documented other
/// half of the same workaround.
///
/// # Safety
/// Must be called on the thread that owns `hwnd`.
pub unsafe fn show_menu(hwnd: HWND, prefs: &MenuPrefs) -> Option<usize> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let toggle = if prefs.armed { "Disarm" } else { "Arm" };
        let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE, &HSTRING::from(toggle));
        // Both need a live buffer, so they are greyed rather than hidden while
        // disarmed: the menu keeps one shape, and the reason a click would do
        // nothing is visible.
        let needs_armed = if prefs.armed { MF_STRING } else { MF_STRING | MF_GRAYED };
        let _ = AppendMenuW(menu, needs_armed, ID_CLIP, &HSTRING::from("Save clip now"));
        let _ = AppendMenuW(menu, needs_armed, ID_SCREENSHOT, &HSTRING::from("Take screenshot"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_UI, &HSTRING::from("Open Clippy"));
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_FOLDER, &HSTRING::from("Open clips folder"));
        // Directly under "Open clips folder": the two are the same subject, and
        // a user who just looked at where clips land is the one who wants to
        // move them. The ellipsis is the Windows convention for "this opens a
        // dialog rather than doing something now".
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_CHANGE_FOLDER,
            &HSTRING::from("Change clips folder..."),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Changing any of these three while armed re-arms (see
        // `window::handle_action`): the encoded size, rate and ring length are
        // fixed when the session opens.
        let resolutions: Vec<(String, bool)> = RESOLUTION_CHOICES
            .iter()
            .map(|(label, value)| {
                (label.to_string(), prefs.resolution.eq_ignore_ascii_case(value))
            })
            .collect();
        append_choices(menu, "Resolution", ID_RESOLUTION_BASE, &resolutions);
        let rates: Vec<(String, bool)> =
            FPS_CHOICES.iter().map(|f| (format!("{f} fps"), prefs.fps == *f)).collect();
        append_choices(menu, "Frame rate", ID_FPS_BASE, &rates);
        let lengths: Vec<(String, bool)> = REPLAY_CHOICES
            .iter()
            .map(|s| (format!("{s} seconds"), prefs.replay_seconds == *s))
            .collect();
        append_choices(menu, "Replay length", ID_REPLAY_BASE, &lengths);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let _ = AppendMenuW(
            menu,
            item_flags(prefs.autostart),
            ID_AUTOSTART,
            &HSTRING::from("Start with Windows"),
        );
        let _ = AppendMenuW(
            menu,
            item_flags(prefs.auto_arm),
            ID_AUTO_ARM,
            &HSTRING::from("Arm when Clippy starts"),
        );
        let _ = AppendMenuW(
            menu,
            item_flags(prefs.clip_sound),
            ID_CLIP_SOUND,
            &HSTRING::from("Play a sound when a clip is saved"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, &HSTRING::from("Quit"));

        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RIGHTALIGN | TPM_RETURNCMD,
            cursor.x,
            cursor.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu);
        match chosen.0 {
            0 => None,
            id => Some(id as usize),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two states must be visually distinct — an armed daemon that looks
    /// identical to an idle one is the tray icon failing at its only job.
    ///
    /// Asserts the shape rather than a pixel count. The plan's original
    /// `armed_on > idle_on * 2` is arithmetically unreachable: with an inner
    /// radius of `0.55 × outer` the ring covers ~70% of the disc's area, not
    /// under half, so that test could only ever have failed.
    #[test]
    fn armed_and_idle_icons_differ_and_are_both_drawn() {
        let idle = icon_pixels(false);
        let armed = icon_pixels(true);
        assert_eq!(idle.len(), ICON_SIDE * ICON_SIDE, "one BGRA pixel per cell");
        assert_eq!(armed.len(), idle.len());

        let alpha = |px: &[u32], x: usize, y: usize| px[y * ICON_SIDE + x] >> 24;
        let opaque = |px: &[u32]| px.iter().filter(|p| (*p >> 24) > 0x40).count();
        let mid = ICON_SIDE / 2;

        // The entire difference between the two states, in two assertions:
        // the middle is empty when idle and filled when armed.
        assert!(alpha(&idle, mid, mid) < 0x20, "the idle icon must be hollow in the middle");
        assert!(alpha(&armed, mid, mid) > 0xE0, "the armed icon must be filled in the middle");

        // Same silhouette either way, so the icon still reads as Clippy in both
        // states — only the fill changes.
        let rim = mid + (ICON_SIDE as f32 * 0.44) as usize - 1;
        assert!(alpha(&idle, rim, mid) > 0x40, "the idle icon must draw its rim");
        assert!(alpha(&armed, rim, mid) > 0x40, "the armed icon must draw its rim");
        assert!(opaque(&armed) > opaque(&idle), "filled must cover more than hollow");

        // A fully opaque corner means the alpha channel was lost somewhere,
        // which shows up in the tray as a white square.
        assert_eq!(alpha(&idle, 0, 0), 0, "corners must be transparent");
        assert_eq!(alpha(&armed, 0, 0), 0, "corners must be transparent");
    }

    /// A click on a submenu item must become exactly the config change its
    /// label names, and ids outside the tables must map to nothing -- the
    /// plain menu ids (1..=10) sit below the bases.
    #[test]
    fn submenu_ids_map_to_the_config_change_they_label() {
        assert_eq!(choice_for_id(ID_RESOLUTION_BASE), Some(("resolution", Value::from("native"))));
        assert_eq!(
            choice_for_id(ID_RESOLUTION_BASE + 2),
            Some(("resolution", Value::from("720p")))
        );
        assert_eq!(choice_for_id(ID_FPS_BASE + 4), Some(("fps", Value::from(120))));
        assert_eq!(choice_for_id(ID_REPLAY_BASE + 1), Some(("replay_seconds", Value::from(15))));
        for plain in [ID_TOGGLE, ID_OPEN_UI, ID_QUIT, ID_CLIP, ID_AUTOSTART, ID_CLIP_SOUND] {
            assert_eq!(choice_for_id(plain), None, "id {plain} is not a choice");
        }
        assert_eq!(choice_for_id(ID_RESOLUTION_BASE + RESOLUTION_CHOICES.len()), None);
        assert_eq!(choice_for_id(ID_FPS_BASE + FPS_CHOICES.len()), None);
        assert_eq!(choice_for_id(ID_REPLAY_BASE + REPLAY_CHOICES.len()), None);
    }

    /// Every fps and replay choice must be a value `config.set` accepts, or the
    /// menu would offer an item that can only fail.
    #[test]
    fn every_offered_choice_is_inside_the_documented_bounds() {
        for f in FPS_CHOICES {
            assert!((clippy_core::config::MIN_FPS..=clippy_core::config::MAX_FPS).contains(&f));
        }
        for s in REPLAY_CHOICES {
            assert!((1..=600).contains(&s));
        }
    }
}
