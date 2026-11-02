//! The tray icon, its menu, and the armed/idle artwork (spec §7.2).
//!
//! Everything here runs on the window's pump thread. Tray icons belong to the
//! thread that owns the window they post to, and `TrackPopupMenu` runs its own
//! modal loop that only works on a thread with a message queue — so the `Tray`
//! is created, updated, and destroyed by `window.rs`'s pump and never touched
//! from the worker. The worker asks for a state change by posting a message.

use anyhow::{Context as _, Result, anyhow};
use serde_json::Value;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, HMENU,
    MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTALIGN, TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

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
const ID_CAPTURE_MODE_BASE: usize = 400;

/// What the capture mode submenu offers: a label and the config value it writes.
pub const CAPTURE_MODE_CHOICES: [(&str, &str); 2] =
    [("Jeux et bureau", "desktop"), ("Jeux uniquement", "game")];

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
    /// On, but idle until a game is in front.
    pub waiting: bool,
    pub capture_mode: String,
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
    if let Some(i) = index(ID_CAPTURE_MODE_BASE, CAPTURE_MODE_CHOICES.len()) {
        return Some(("capture_mode", Value::from(CAPTURE_MODE_CHOICES[i].1)));
    }
    None
}

/// Icon edge in pixels. 32 is the large-DPI tray size; Windows downscales to
/// 16 cleanly and asking for a 16 would look soft at 150% scaling.
pub(crate) const ICON_SIDE: usize = 32;

/// The app's logo -- the same two interlocking loops as in the window, the
/// taskbar and the shortcuts -- as premultiplied BGRA: grey when idle, the
/// app's blue when the replay is armed.
///
/// The shape is the alpha channel of the 32 px app icon (`icons/32x32.png`,
/// generated from `crates/clippy-ui/icon-source.svg`), stored raw so no image
/// decoder is needed. Both colours read on light and dark taskbars, and the
/// change of colour is the armed/idle state.
pub(crate) fn icon_pixels(armed: bool) -> Vec<u32> {
    // #2b7fff armed, #8f8f8f idle.
    let (r, g, b) = if armed { (0x2b, 0x7f, 0xff) } else { (0x8f, 0x8f, 0x8f) };
    TRAY_LOGO
        .iter()
        .map(|&a| {
            let a = u32::from(a);
            // Premultiplied: the tray composites over unknown backgrounds and
            // a non-premultiplied icon fringes dark.
            let pm = |c: u32| c * a / 255;
            (a << 24) | (pm(r) << 16) | (pm(g) << 8) | pm(b)
        })
        .collect()
}

/// One alpha byte per pixel, row by row, `ICON_SIDE` x `ICON_SIDE`.
static TRAY_LOGO: &[u8; ICON_SIDE * ICON_SIDE] = include_bytes!("../assets/tray-logo.a8");

/// A live tray icon. Removing it is not optional: an icon whose process dies
/// without `NIM_DELETE` lingers in the tray until the user happens to hover
/// it, which looks exactly like a crashed app.
pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 2],
    armed: bool,
    /// The replay is on but idle, waiting for a game (`game` capture mode).
    waiting: bool,
}

impl Tray {
    /// # Safety
    /// Must be called on the thread that owns `hwnd`, and the `Tray` must be
    /// dropped on that same thread.
    pub unsafe fn add(hwnd: HWND) -> Result<Self> {
        let icons = [unsafe { make_icon(false) }?, unsafe { make_icon(true) }?];
        let tray = Self { hwnd, icons, armed: false, waiting: false };
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
    pub fn set_state(&mut self, armed: bool, waiting: bool) {
        if self.armed == armed && self.waiting == waiting {
            return;
        }
        self.armed = armed;
        self.waiting = waiting;
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
        let tip = match (self.armed, self.waiting) {
            (true, _) => "Clippy — replay activé",
            (false, true) => "Clippy — en attente d’un jeu",
            (false, false) => "Clippy — replay désactivé",
        };
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

/// Dark menus, like the app, instead of the light ones Windows gives a
/// process by default.
///
/// There is no documented switch for a Win32 menu's theme. These are the
/// `uxtheme.dll` exports Explorer, Notepad++ and the Windows Terminal use:
/// ordinal 135 `SetPreferredAppMode(ForceDark = 2)` and ordinal 136
/// `FlushMenuThemes`, stable since Windows 10 1903. Before 1903, 135 was
/// `AllowDarkModeForApp(BOOL)`, which a 2 merely turns on; if either is
/// missing the menu stays light. Once per process: the mode is process-wide.
fn use_dark_menus() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        let ordinal = |n: usize| GetProcAddress(uxtheme, PCSTR(n as *const u8));
        let (Some(set_mode), Some(flush)) = (ordinal(135), ordinal(136)) else { return };
        // SAFETY: the signatures of those two exports, see above.
        let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
        let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
        set_mode(2);
        flush();
    });
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
    use_dark_menus();
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let toggle = match (prefs.armed, prefs.waiting) {
            (true, _) => "Désactiver le replay",
            (false, true) => "Désactiver le replay (en attente d’un jeu)",
            (false, false) => "Activer le replay",
        };
        let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE, &HSTRING::from(toggle));
        // Both need a live buffer, so they are greyed rather than hidden while
        // disarmed: the menu keeps one shape, and the reason a click would do
        // nothing is visible.
        let needs_armed = if prefs.armed { MF_STRING } else { MF_STRING | MF_GRAYED };
        let _ = AppendMenuW(
            menu,
            needs_armed,
            ID_CLIP,
            &HSTRING::from("Enregistrer un clip maintenant"),
        );
        let _ = AppendMenuW(
            menu,
            needs_armed,
            ID_SCREENSHOT,
            &HSTRING::from("Prendre une capture d’écran"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_UI, &HSTRING::from("Ouvrir Clippy"));
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_OPEN_FOLDER,
            &HSTRING::from("Ouvrir le dossier des clips"),
        );
        // Directly under "Open clips folder": the two are the same subject, and
        // a user who just looked at where clips land is the one who wants to
        // move them. The ellipsis is the Windows convention for "this opens a
        // dialog rather than doing something now".
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_CHANGE_FOLDER,
            &HSTRING::from("Changer le dossier des clips…"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Changing any of these three while armed re-arms (see
        // `window::handle_action`): the encoded size, rate and ring length are
        // fixed when the session opens.
        let resolutions: Vec<(String, bool)> = RESOLUTION_CHOICES
            .iter()
            .map(|(label, value)| (label.to_string(), prefs.resolution.eq_ignore_ascii_case(value)))
            .collect();
        append_choices(menu, "Résolution", ID_RESOLUTION_BASE, &resolutions);
        let rates: Vec<(String, bool)> =
            FPS_CHOICES.iter().map(|f| (format!("{f} fps"), prefs.fps == *f)).collect();
        append_choices(menu, "Fréquence d’images", ID_FPS_BASE, &rates);
        let lengths: Vec<(String, bool)> = REPLAY_CHOICES
            .iter()
            .map(|s| (format!("{s} secondes"), prefs.replay_seconds == *s))
            .collect();
        append_choices(menu, "Durée du replay", ID_REPLAY_BASE, &lengths);
        // Takes effect at once, with no re-arm: the daemon starts or pauses the
        // replay to match.
        let modes: Vec<(String, bool)> = CAPTURE_MODE_CHOICES
            .iter()
            .map(|(label, value)| {
                (label.to_string(), prefs.capture_mode.eq_ignore_ascii_case(value))
            })
            .collect();
        append_choices(menu, "Mode de capture", ID_CAPTURE_MODE_BASE, &modes);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let _ = AppendMenuW(
            menu,
            item_flags(prefs.autostart),
            ID_AUTOSTART,
            &HSTRING::from("Lancer au démarrage de Windows"),
        );
        let _ = AppendMenuW(
            menu,
            item_flags(prefs.auto_arm),
            ID_AUTO_ARM,
            &HSTRING::from("Activer le replay au lancement de Clippy"),
        );
        let _ = AppendMenuW(
            menu,
            item_flags(prefs.clip_sound),
            ID_CLIP_SOUND,
            &HSTRING::from("Jouer un son à l’enregistrement d’un clip"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, &HSTRING::from("Quitter"));

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

    /// Both states draw the logo, and an armed icon must not look like an idle
    /// one: same silhouette, different colour.
    #[test]
    fn armed_and_idle_icons_differ_and_are_both_drawn() {
        let idle = icon_pixels(false);
        let armed = icon_pixels(true);
        assert_eq!(idle.len(), ICON_SIDE * ICON_SIDE, "one BGRA pixel per cell");
        assert_eq!(armed.len(), idle.len());

        let alpha = |p: u32| p >> 24;
        let drawn = idle.iter().filter(|p| alpha(**p) > 0x40).count();
        assert!(drawn > ICON_SIDE * ICON_SIDE / 5, "the logo must be drawn, not a speck ({drawn} px)");
        assert!(idle.iter().zip(&armed).all(|(i, a)| alpha(*i) == alpha(*a)), "same silhouette in both states");

        // The colour is the state: armed is blue, idle is grey.
        let solid = |px: &[u32]| *px.iter().find(|p| alpha(**p) == 0xFF).expect("an opaque pixel");
        let (blue, grey) = (solid(&armed), solid(&idle));
        assert!((blue & 0xFF) > ((blue >> 16) & 0xFF) + 0x80, "armed must be blue");
        assert_eq!(grey & 0xFF, (grey >> 16) & 0xFF, "idle must be grey");

        // Premultiplied: no channel above alpha, or the tray draws fringes.
        assert!(armed.iter().all(|p| (0..3).all(|c| ((p >> (8 * c)) & 0xFF) <= alpha(*p))));

        // A fully opaque corner means the alpha channel was lost somewhere,
        // which shows up in the tray as a coloured square.
        assert_eq!(alpha(idle[0]), 0, "corners must be transparent");
        assert_eq!(alpha(armed[ICON_SIDE * ICON_SIDE - 1]), 0, "corners must be transparent");
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
        assert_eq!(
            choice_for_id(ID_CAPTURE_MODE_BASE + 1),
            Some(("capture_mode", Value::from("game")))
        );
        assert_eq!(choice_for_id(ID_CAPTURE_MODE_BASE + CAPTURE_MODE_CHOICES.len()), None);
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

/// A look at the real menu: opens it, photographs the screen around it from a
/// timer on the menu's own thread, and closes it with `EndMenu` -- no key or
/// click is sent anywhere.
#[cfg(test)]
mod preview {
    use super::*;
    use std::sync::OnceLock;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, EndMenu, KillTimer, SetTimer, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    static OUT: OnceLock<String> = OnceLock::new();

    unsafe extern "system" fn snap(hwnd: HWND, _: u32, id: usize, _: u32) {
        unsafe {
            let _ = KillTimer(Some(hwnd), id);
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let (w, h) = (520, 760);
            let (x, y) = (cursor.x - w + 20, cursor.y - h / 2);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bitmap.into());
            let _ = BitBlt(dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            GetDIBits(dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            let mut out = Vec::new();
            out.extend_from_slice(&(w as u32).to_le_bytes());
            out.extend_from_slice(&(h as u32).to_le_bytes());
            out.extend_from_slice(&pixels);
            let _ = std::fs::write(OUT.get().unwrap(), out);
            let _ = EndMenu();
        }
    }

    #[test]
    #[ignore = "opens the tray menu on screen for a second; run by hand"]
    fn menu() {
        let Ok(path) = std::env::var("CLIPPY_MENU_PREVIEW") else { return };
        OUT.set(path).unwrap();
        unsafe {
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!(""), WS_POPUP, 0, 0, 0, 0, None, None, None, None)
                .expect("window");
            SetTimer(Some(hwnd), 7, 900, Some(snap));
            let prefs = MenuPrefs {
                armed: true,
                waiting: false,
                capture_mode: "desktop".into(),
                autostart: true,
                auto_arm: true,
                clip_sound: true,
                resolution: "native".into(),
                fps: 120,
                replay_seconds: 60,
            };
            show_menu(hwnd, &prefs);
        }
    }
}
