//! The on-screen banner shown when a clip or a screenshot is saved, the way
//! GeForce Experience / the NVIDIA app and the Xbox Game Bar confirm one.
//!
//! A topmost layered popup in the top-right corner of the monitor the
//! foreground window is on. Four properties matter more than the looks:
//!
//! - **It never takes focus.** `WS_EX_NOACTIVATE`, `SW_SHOWNOACTIVATE` and
//!   `SWP_NOACTIVATE`: a game that loses focus mid-fight minimises or drops
//!   input, which is far worse than no banner.
//! - **It is click-through.** `WS_EX_TRANSPARENT` plus `WS_EX_LAYERED`, so a
//!   click on that corner reaches the game.
//! - **It is not recorded.** `WDA_EXCLUDEFROMCAPTURE` keeps it out of Windows
//!   Graphics Capture, so it never shows up in the next clip's replay buffer.
//! - **It never blocks the caller.** [`show`] stores the text and posts a
//!   message; the window lives on a thread of its own with its own pump.
//!
//! It cannot appear over a game in *exclusive* fullscreen (nothing but the
//! game's own swap chain is composed then); borderless and windowed games,
//! which is what almost every modern game defaults to, show it.
//!
//! Rendering: GDI draws the opaque card (background, glyph, text) into a 32-bit
//! DIB, then every pixel's alpha is set from an anti-aliased rounded-rectangle
//! mask and premultiplied, and the result goes to `UpdateLayeredWindow`. GDI's
//! own alpha handling is never relied on, which is why the text can use
//! ClearType on the opaque fill.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    CreateSolidBrush, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FillRect, GdiFlush, GetDC,
    GetMonitorInfoW, HBITMAP, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromWindow, OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    HWND_TOPMOST, KillTimer, MSG, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WM_APP, WM_TIMER,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

/// What the banner says. `title` is the bold line, `detail` the grey one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub detail: String,
}

const WM_SHOW: u32 = WM_APP + 1;
const TIMER: usize = 1;
const FRAME: Duration = Duration::from_millis(16);
const FADE_IN: Duration = Duration::from_millis(180);
const HOLD: Duration = Duration::from_millis(2800);
const FADE_OUT: Duration = Duration::from_millis(320);
/// Peak opacity: a hair under opaque so a bright game scene reads through a
/// little and the card feels like an overlay, not a dialog.
const OPACITY: f32 = 0.96;

static HWND_BITS: AtomicIsize = AtomicIsize::new(0);
static PENDING: Mutex<Option<Message>> = Mutex::new(None);
static START: Once = Once::new();

/// Shows `message`, replacing whatever the banner currently says and
/// restarting its timer. Never blocks and never fails: a banner is feedback
/// about something that already happened.
pub fn show(message: Message) {
    tracing::debug!(title = %message.title, detail = %message.detail, "banner");
    if let Ok(mut pending) = PENDING.lock() {
        *pending = Some(message);
    }
    START.call_once(|| {
        if let Err(e) = std::thread::Builder::new().name("clippy-banner".into()).spawn(pump) {
            tracing::warn!(error = %e, "could not start the banner thread");
        }
    });
    let bits = HWND_BITS.load(Ordering::Acquire);
    if bits != 0 {
        // A failed post only loses this one banner.
        let _ = unsafe { PostMessageW(Some(HWND(bits as _)), WM_SHOW, WPARAM(0), LPARAM(0)) };
    }
}

/// "0:15" for 15 000 ms, "1:02:03" past an hour.
pub fn duration_label(ms: u64) -> String {
    let total = ms.div_ceil(1000);
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

struct Shown {
    started: Instant,
    /// Final position of the card's top-left corner, and its size.
    at: POINT,
    size: SIZE,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}

impl Drop for Shown {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

thread_local! {
    static SHOWN: std::cell::RefCell<Option<Shown>> = const { std::cell::RefCell::new(None) };
}

fn pump() {
    unsafe {
        // Without this the card is laid out in 96-dpi pixels and then
        // bitmap-stretched by Windows on a 150 % screen: blurry text.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: module.into(),
            lpszClassName: w!("ClippyBanner"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            tracing::warn!("could not register the banner window class");
            return;
        }
        let hwnd = match CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ClippyBanner"),
            w!("Clippy"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(e) => {
                tracing::warn!(error = %e, "could not create the banner window");
                return;
            }
        };
        // Windows 10 2004 and later. On older builds it fails and the banner
        // can be recorded, which is cosmetic.
        // `CLIPPY_BANNER_CAPTURABLE` exists for screenshot tests only: the
        // affinity hides the window from `BitBlt` as well as from WGC.
        if std::env::var_os("CLIPPY_BANNER_CAPTURABLE").is_some() {
            tracing::debug!("banner left capturable for a test");
        } else if let Err(e) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
            tracing::debug!(error = %e, "banner stays visible to screen capture");
        }
        HWND_BITS.store(hwnd.0 as isize, Ordering::Release);
        // A `show` that ran before the window existed left its text pending.
        let _ = PostMessageW(Some(hwnd), WM_SHOW, WPARAM(0), LPARAM(0));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SHOW => {
            let message = PENDING.lock().ok().and_then(|mut p| p.take());
            if let Some(message) = message {
                SHOWN.with(|cell| {
                    // Drop the previous card's GDI objects before building the next.
                    let was_showing = cell.borrow_mut().take().is_some();
                    let mut next = unsafe { build(&message) };
                    // A banner already on screen changes its text in place
                    // rather than sliding in a second time (an automatic clip
                    // renames the plain "Clip enregistré" a moment later).
                    if was_showing && let Some(shown) = next.as_mut() {
                        shown.started = Instant::now().checked_sub(FADE_IN).unwrap_or(shown.started);
                    }
                    *cell.borrow_mut() = next;
                });
                unsafe {
                    let _ = SetTimer(Some(hwnd), TIMER, FRAME.as_millis() as u32, None);
                }
                unsafe { tick(hwnd) };
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER => {
            unsafe { tick(hwnd) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Advances the animation one frame, hiding the window once it is over.
unsafe fn tick(hwnd: HWND) {
    SHOWN.with(|cell| {
        let mut slot = cell.borrow_mut();
        // `None` also when `build` failed: hide and stop the timer rather than
        // leave the previous card up and the timer firing forever.
        let Some(frame) = slot.as_ref().and_then(|shown| frame_at(shown.started.elapsed())) else {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            slot.take();
            return;
        };
        let Some(shown) = slot.as_ref() else { return };
        let scale = shown.size.cy as f32 / BASE_HEIGHT as f32;
        let at = POINT { x: shown.at.x + (frame.slide * 18.0 * scale) as i32, y: shown.at.y };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (frame.opacity * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            let screen = GetDC(None);
            let origin = POINT { x: 0, y: 0 };
            let result = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&at),
                Some(&shown.size),
                Some(shown.dc),
                Some(&origin),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
            if let Err(e) = result {
                tracing::debug!(error = %e, "banner frame not drawn");
            }
            // Re-asserted every frame: a game that goes topmost itself after
            // the banner appeared would otherwise cover it.
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    });
}

#[derive(Debug, PartialEq)]
struct Frame {
    opacity: f32,
    /// 1 = fully offset to the right, 0 = in place.
    slide: f32,
}

/// The animation as a pure function of time, so it can be tested: an ease-out
/// fade and slide in, a hold, a fade out, then `None` (hide).
fn frame_at(t: Duration) -> Option<Frame> {
    let ease_out = |x: f32| 1.0 - (1.0 - x).powi(3);
    if t < FADE_IN {
        let p = ease_out(t.as_secs_f32() / FADE_IN.as_secs_f32());
        return Some(Frame { opacity: OPACITY * p, slide: 1.0 - p });
    }
    let t = t - FADE_IN;
    if t < HOLD {
        return Some(Frame { opacity: OPACITY, slide: 0.0 });
    }
    let t = t - HOLD;
    if t < FADE_OUT {
        let p = t.as_secs_f32() / FADE_OUT.as_secs_f32();
        return Some(Frame { opacity: OPACITY * (1.0 - p * p), slide: 0.0 });
    }
    None
}

/// Card size at 96 dpi.
const BASE_WIDTH: i32 = 340;
const BASE_HEIGHT: i32 = 64;
const BASE_MARGIN: i32 = 24;
const BASE_RADIUS: f32 = 12.0;

/// GDI colours are 0x00BBGGRR.
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const BACKGROUND: (u8, u8, u8) = (24, 24, 27);
const BORDER: (u8, u8, u8) = (63, 63, 70);
const ACCENT: (u8, u8, u8) = (59, 130, 246);

/// Lays the card out for the monitor the foreground window is on and renders
/// it into a premultiplied 32-bit DIB.
unsafe fn build(message: &Message) -> Option<Shown> {
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = dpi_x.max(96) as f32 / 96.0;
        let px = |v: i32| (v as f32 * scale).round() as i32;
        let (width, height) = (px(BASE_WIDTH), px(BASE_HEIGHT));
        let area = info.rcMonitor;
        let at = POINT { x: area.right - width - px(BASE_MARGIN), y: area.top + px(BASE_MARGIN) };

        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        ReleaseDC(None, screen);
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: top-down rows, so pixel (x, y) is at y * width + x.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let old = SelectObject(dc, bitmap.into());

        // Opaque card.
        let brush = CreateSolidBrush(rgb(BACKGROUND.0, BACKGROUND.1, BACKGROUND.2));
        FillRect(dc, &RECT { left: 0, top: 0, right: width, bottom: height }, brush);
        let _ = DeleteObject(brush.into());
        // Accent stripe on the left edge (clipped round by the mask below).
        let accent = CreateSolidBrush(rgb(ACCENT.0, ACCENT.1, ACCENT.2));
        FillRect(dc, &RECT { left: 0, top: 0, right: px(4), bottom: height }, accent);
        let _ = DeleteObject(accent.into());

        SetBkMode(dc, TRANSPARENT);
        // The check mark, from the icon font every Windows 10/11 has.
        let glyph_font = font(px(20), 400, w!("Segoe MDL2 Assets"));
        let previous = SelectObject(dc, glyph_font.into());
        SetTextColor(dc, rgb(ACCENT.0, ACCENT.1, ACCENT.2));
        let mut glyph: Vec<u16> = "\u{E73E}".encode_utf16().collect();
        let mut rect = RECT { left: px(18), top: 0, right: px(44), bottom: height };
        DrawTextW(dc, &mut glyph, &mut rect, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);

        let title_font = font(px(15), 600, w!("Segoe UI"));
        SelectObject(dc, title_font.into());
        SetTextColor(dc, rgb(250, 250, 250));
        let mut title: Vec<u16> = message.title.encode_utf16().collect();
        let mut rect = RECT { left: px(52), top: px(12), right: width - px(16), bottom: px(34) };
        DrawTextW(dc, &mut title, &mut rect, DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS);

        let detail_font = font(px(13), 400, w!("Segoe UI"));
        SelectObject(dc, detail_font.into());
        SetTextColor(dc, rgb(161, 161, 170));
        let mut detail: Vec<u16> = message.detail.encode_utf16().collect();
        let mut rect = RECT { left: px(52), top: px(34), right: width - px(16), bottom: px(54) };
        DrawTextW(dc, &mut detail, &mut rect, DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS);

        SelectObject(dc, previous);
        for font in [glyph_font, title_font, detail_font] {
            let _ = DeleteObject(font.into());
        }
        let _ = GdiFlush();

        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (width * height * 4) as usize);
        shape(pixels, width, height, BASE_RADIUS * scale, scale);

        Some(Shown { started: Instant::now(), at, size: SIZE { cx: width, cy: height }, dc, bitmap, old })
    }
}

unsafe fn font(height: i32, weight: i32, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            face,
        )
    }
}

/// Signed distance from (x, y) to a rounded rectangle of `w`×`h` with corner
/// `radius`: negative inside, positive outside.
fn rounded_rect_distance(x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - radius);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// Turns the opaque GDI drawing into a premultiplied BGRA card: alpha from an
/// anti-aliased rounded-rectangle mask, and a one-pixel border blended in
/// along the edge.
fn shape(pixels: &mut [u8], width: i32, height: i32, radius: f32, scale: f32) {
    let border = scale.max(1.0);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let d = rounded_rect_distance(
                x as f32 + 0.5,
                y as f32 + 0.5,
                width as f32,
                height as f32,
                radius,
            );
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            // 1 on the outermost `border` pixels, fading to 0 inside them.
            let edge = (d + border + 0.5).clamp(0.0, 1.0);
            let mix = |channel: u8, line: u8| channel as f32 + (line as f32 - channel as f32) * edge * 0.8;
            let (b, g, r) = (
                mix(pixels[i], BORDER.2),
                mix(pixels[i + 1], BORDER.1),
                mix(pixels[i + 2], BORDER.0),
            );
            pixels[i] = (b * coverage).round() as u8;
            pixels[i + 1] = (g * coverage).round() as u8;
            pixels[i + 2] = (r * coverage).round() as u8;
            pixels[i + 3] = (255.0 * coverage).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_animation_fades_in_holds_fades_out_and_ends() {
        let start = frame_at(Duration::ZERO).unwrap();
        assert_eq!(start.opacity, 0.0);
        assert_eq!(start.slide, 1.0);
        let held = frame_at(FADE_IN + HOLD / 2).unwrap();
        assert_eq!(held, Frame { opacity: OPACITY, slide: 0.0 });
        let leaving = frame_at(FADE_IN + HOLD + FADE_OUT / 2).unwrap();
        assert!(leaving.opacity > 0.0 && leaving.opacity < OPACITY);
        assert!(frame_at(FADE_IN + HOLD + FADE_OUT).is_none());
    }

    #[test]
    fn the_card_mask_is_opaque_inside_clear_outside_and_soft_on_the_corner() {
        let (w, h) = (40, 20);
        let mut pixels = vec![200u8; (w * h * 4) as usize];
        shape(&mut pixels, w, h, 6.0, 1.0);
        let alpha = |x: i32, y: i32| pixels[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(20, 10), 255, "centre");
        assert_eq!(alpha(0, 0), 0, "corner pixel is outside the rounding");
        let soft = alpha(1, 2);
        assert!(soft > 0 && soft < 255, "corner edge is anti-aliased, got {soft}");
        // Premultiplied: no colour channel exceeds alpha.
        for p in pixels.chunks_exact(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
        }
    }

    #[test]
    fn durations_read_like_the_library_does() {
        assert_eq!(duration_label(15_000), "0:15");
        assert_eq!(duration_label(14_200), "0:15");
        assert_eq!(duration_label(62_000), "1:02");
        assert_eq!(duration_label(3_723_000), "1:02:03");
    }
}
