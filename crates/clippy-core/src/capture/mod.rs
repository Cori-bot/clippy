pub mod audio;
mod duplication;
pub mod probe;
pub mod source;
pub mod stage;
pub mod video;
mod wgc;

use std::time::{Duration, Instant};

use windows::Graphics::Capture::{GraphicsCaptureAccess, GraphicsCaptureAccessKind};
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_SCHEDULINGPRIORITYCLASS_BELOW_NORMAL, D3DKMTSetProcessSchedulingPriorityClass,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows_capture::{
    graphics_capture_api::GraphicsCaptureApi,
    settings::{DrawBorderSettings, MinimumUpdateIntervalSettings},
};

/// Chooses the capture-border setting the running OS actually supports.
///
/// The WGC `GraphicsCaptureSession.IsBorderRequired` property — which
/// suppresses the yellow "you are being captured" border — only exists on
/// Windows 11 / Windows Server 2022 (build 20348) and newer. Consumer
/// Windows 10 (through build 19045 / 22H2) does not expose it, and
/// `windows-capture` rejects any non-`Default` border setting there with
/// `BorderConfigUnsupported`, aborting capture start entirely.
///
/// So query support first: hide the border where the platform allows it,
/// otherwise fall back to the OS default (border shown) so capture still
/// runs. This is an OS-build gate, not a GPU/vendor issue.
///
/// Support is necessary and not sufficient. Windows also requires consent
/// before it acts on the property, and withholds it silently — see
/// [`request_borderless_consent`], which is why asking for it happens here,
/// on the way to every session rather than once per launch.
pub(super) fn border_settings() -> DrawBorderSettings {
    match GraphicsCaptureApi::is_border_settings_supported() {
        Ok(true) => {
            request_borderless_consent();
            tracing::info!("capture border: asking Windows to hide it");
            DrawBorderSettings::WithoutBorder
        }
        supported => {
            tracing::info!(
                ?supported,
                "capture border: this Windows build cannot hide it, leaving the OS default"
            );
            DrawBorderSettings::Default
        }
    }
}

/// Asks Windows for the consent that `IsBorderRequired = false` needs before
/// the system will act on it.
///
/// `windows-capture` sets the property but never requests this, and Microsoft
/// documents the omission as failing silently: without consent, "setting this
/// property to false will succeed, but the value will be ignored and the
/// border will be displayed". So the border can come back with no error
/// raised and nothing in the log to show for it — which is exactly the report
/// this exists to answer.
///
/// Asked on every session start rather than once per launch: the replay engine
/// rebuilds its session whenever the display topology changes under it (a game
/// going fullscreen does precisely that, see `run_driven_inner`), and a
/// rebuilt session that never re-asked is the suspect case.
///
/// Every outcome is logged and swallowed. Consent only governs what the user
/// sees on their own screen — it is never a reason to refuse to record, and a
/// refusal here must not take the capture down with it.
fn request_borderless_consent() {
    let request =
        match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless) {
            Ok(request) => request,
            Err(e) => {
                tracing::warn!("could not ask for borderless capture consent: {e}");
                return;
            }
        };

    // Settle the request by retrying `GetResults`, which refuses until the
    // operation completes. `IAsyncOperation::get` would say this directly, but
    // windows-future 0.3 dropped it, and polling `Status` properly would mean
    // naming `AsyncStatus` — which `windows` does not re-export, so it would
    // cost a new direct dependency for one enum.
    //
    // Consent already recorded for this app answers on the first try, so the
    // common path does not sleep at all. The deadline is short because a
    // request that has not settled by then is one Windows is putting in front
    // of the user, and arming must not wait on that: the grant still lands for
    // the next session either way.
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut pending = None;
    while Instant::now() < deadline {
        match request.GetResults() {
            Ok(status) if status == AppCapabilityAccessStatus::Allowed => {
                tracing::info!("borderless capture consent granted");
                return;
            }
            Ok(status) => {
                tracing::warn!(
                    ?status,
                    "borderless capture consent not granted — Windows will draw the capture border"
                );
                return;
            }
            Err(e) => pending = Some(e),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    tracing::warn!(?pending, "borderless capture consent did not settle in time");
}

/// Asks WGC not to deliver frames faster than ~4/3 of the target fps, where
/// the OS supports it (`GraphicsCaptureSession.MinUpdateInterval`, Windows 11
/// 24H2+; same runtime-gate pattern as [`border_settings`]).
///
/// On high-refresh monitors this stops DWM from copying 144/165 frames per
/// second into the frame pool when the encoder only wants 60. The interval is
/// deliberately 3/4 of a frame period — a full period would beat against the
/// compositor's own cadence and halve the delivery rate; exact pacing to the
/// target fps is done by the sessions' QPC pacer instead.
pub(super) fn min_update_interval(fps: u32, refresh_hz: u32) -> MinimumUpdateIntervalSettings {
    match GraphicsCaptureApi::is_minimum_update_interval_supported() {
        Ok(true) => {
            // `Default` is NOT "uncapped": measured on a 144 Hz display it
            // delivers ~48 fps (one vsync in three). So when 3/4 of a period
            // would quantize below the target, ask for half a vsync instead,
            // which lets every refresh through and leaves the thinning to the
            // sinks' QPC pacer.
            let interval_us = if update_interval_cap_is_safe(fps, refresh_hz) {
                750_000 / u64::from(fps.max(1))
            } else {
                500_000 / u64::from(refresh_hz)
            };
            let interval = Duration::from_micros(interval_us);
            tracing::info!(?interval, fps, refresh_hz, "capping WGC frame delivery");
            MinimumUpdateIntervalSettings::Custom(interval)
        }
        _ => MinimumUpdateIntervalSettings::Default,
    }
}

/// WGC delivers on display refreshes, so a minimum update interval is really
/// "every m-th vsync" with `m = ceil(interval / vsync_period)`. The cap is only
/// safe if that still reaches the target: at 144 Hz and 90 fps the interval
/// (8.3 ms) is longer than a vsync (6.9 ms), so m = 2 and delivery drops to
/// 72 fps -- measured: 74-77 fps at every resolution. When it is not safe, the
/// QPC pacer in the sinks does all the thinning on an uncapped stream.
/// `refresh_hz == 0` (unknown) keeps the cap, as before.
fn update_interval_cap_is_safe(fps: u32, refresh_hz: u32) -> bool {
    if refresh_hz == 0 || fps == 0 {
        return true;
    }
    let (fps, refresh) = (u64::from(fps), u64::from(refresh_hz));
    // 3/4 of a period over a vsync period, rounded up: ceil(0.75 * refresh / fps).
    let m = (3 * refresh).div_ceil(4 * fps).max(1);
    // refresh / m must reach 99 % of the target.
    100 * refresh >= 99 * fps * m
}

/// Drops this process's GPU scheduling priority below normal so the game's
/// render work always preempts our BGRA→NV12 blits and encode submissions.
/// On a single-GPU machine that turns GPU contention into (rare) capture
/// frame drops instead of lost game fps. Failure is logged and ignored —
/// capture just runs at normal priority.
pub fn lower_gpu_priority() {
    let status = unsafe {
        D3DKMTSetProcessSchedulingPriorityClass(
            GetCurrentProcess(),
            D3DKMT_SCHEDULINGPRIORITYCLASS_BELOW_NORMAL,
        )
    };
    if status.is_ok() {
        tracing::info!("GPU scheduling priority lowered (game work preempts capture)");
    } else {
        tracing::warn!(?status, "could not lower GPU scheduling priority");
    }
}

#[cfg(test)]
mod tests {
    use super::update_interval_cap_is_safe as safe;

    #[test]
    fn the_wgc_cap_is_dropped_when_it_would_quantize_below_the_target() {
        // 144 Hz: delivery steps are 144, 72, 48...
        assert!(safe(60, 144), "60 fps is reached by every second vsync (72)");
        assert!(safe(72, 144));
        assert!(!safe(73, 144));
        assert!(!safe(90, 144), "the measured 74-77 fps case");
        assert!(safe(120, 144), "interval is shorter than a vsync: every vsync passes");
        assert!(!safe(40, 60), "m = 2 would deliver 30");
        assert!(safe(90, 0), "unknown refresh keeps the previous behaviour");
    }
}
