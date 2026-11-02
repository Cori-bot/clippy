//! WASAPI shared-mode capture.
//!
//! Event-driven shared-mode capture: the thread sleeps in the kernel until
//! the audio engine signals a period, then drains all pending packets. Each
//! packet carries a QPC timestamp (100 ns units) — the same clock domain as
//! the video frames, which is what the muxer uses to keep the two in sync.

use std::{
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode,
    WaveFormat,
};

use super::{AudioPacket, CHANNELS, SAMPLE_RATE, downmix::Downmix};

/// Which device a capture session reads.
///
/// Both are shared-mode *capture* clients — the difference is only which
/// default device is enumerated. Initializing a capture client against a
/// **render** device is what makes it a loopback stream; against a **capture**
/// device it is an ordinary microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSourceKind {
    SystemAudio,
    Microphone,
    /// The sound of one process and everything it started, wherever it plays
    /// it. Needs Windows 10 build 20348 or later; on anything older the stream
    /// fails to open and the mixer carries on without it.
    Application {
        pid: u32,
    },
}

impl AudioSourceKind {
    fn direction(self) -> Direction {
        match self {
            // An application stream has no device of its own to pick; it is a
            // loopback, so it reads like one.
            Self::SystemAudio | Self::Application { .. } => Direction::Render,
            Self::Microphone => Direction::Capture,
        }
    }

    /// User-facing name, used in the warning shown when a source is missing.
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemAudio => "system audio",
            Self::Microphone => "microphone",
            Self::Application { .. } => "application audio",
        }
    }
}

/// An event-driven shared-mode capture session on whichever default device
/// [`AudioSourceKind::direction`] resolves to. "Loopback" describes only the
/// system-audio case (a capture client initialized against the render
/// device); against a capture device this is an ordinary microphone stream.
struct WasapiSession {
    client: AudioClient,
    event: Handle,
    capture: AudioCaptureClient,
    block_align: usize,
    scratch: Vec<u8>,
    /// Set when the stream runs in the device's surround layout (f32): its
    /// packets are folded to stereo here, see [`super::downmix`].
    downmix: Option<Downmix>,
    /// The caller's sample format, which a folded packet is written back in.
    out_float: bool,
}

impl WasapiSession {
    /// `format` is what the caller reads: stereo at [`SAMPLE_RATE`]. On a
    /// surround device the stream is opened in the device's own layout
    /// instead and folded to that, because the engine's own downmix costs
    /// ~13.5 dB (see [`super::downmix`]).
    fn open(kind: AudioSourceKind, format: &WaveFormat) -> Result<Self> {
        let out_float = matches!(format.get_subformat(), Ok(SampleType::Float));
        if !matches!(kind, AudioSourceKind::Application { .. }) {
            let mix = Self::client(kind)?.get_mixformat().ok();
            if let Some(mix) = mix.filter(|m| usize::from(m.get_nchannels()) > CHANNELS) {
                let channels = usize::from(mix.get_nchannels());
                let downmix = Downmix::new(channels, mix.get_dwchannelmask());
                let native = WaveFormat::new(
                    32,
                    32,
                    &SampleType::Float,
                    SAMPLE_RATE,
                    channels,
                    Some(mix.get_dwchannelmask()),
                );
                match Self::init(kind, &native) {
                    Ok(mut session) => {
                        tracing::debug!(
                            source = kind.label(),
                            channels,
                            mask = mix.get_dwchannelmask(),
                            "capturing the surround layout and folding it to stereo"
                        );
                        session.downmix = Some(downmix);
                        session.out_float = out_float;
                        return Ok(session);
                    }
                    Err(e) => {
                        tracing::warn!(source = kind.label(), channels, error = %format!("{e:#}"), "surround capture refused; letting Windows downmix (quieter)")
                    }
                }
            }
        }
        let mut session = Self::init(kind, format)?;
        session.out_float = out_float;
        Ok(session)
    }

    /// The caller's samples from one packet: as read, or folded to stereo.
    fn samples(&self, frames: usize) -> Vec<u8> {
        let raw = &self.scratch[..frames * self.block_align];
        let Some(downmix) = &self.downmix else { return raw.to_vec() };
        let input: Vec<f32> =
            raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let mut stereo = Vec::with_capacity(frames * CHANNELS);
        downmix.fold(&input[..frames * downmix.channels()], &mut stereo);
        if self.out_float {
            stereo.iter().flat_map(|s| s.to_le_bytes()).collect()
        } else {
            // Same rounding as the engine's own float-to-PCM conversion.
            stereo
                .iter()
                .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
                .collect()
        }
    }

    fn client(kind: AudioSourceKind) -> Result<AudioClient> {
        if let AudioSourceKind::Application { pid } = kind {
            // Include the process tree: Spotify and every browser play their
            // sound from a child of the process the user knows.
            AudioClient::new_application_loopback_client(pid, true)
                .map_err(|e| anyhow!("application audio client (pid {pid}): {e}"))
        } else {
            let enumerator =
                DeviceEnumerator::new().map_err(|e| anyhow!("device enumerator: {e}"))?;
            let device = enumerator
                .get_default_device(&kind.direction())
                .map_err(|e| anyhow!("no default {} device: {e}", kind.label()))?;
            Ok(device.get_iaudioclient().map_err(|e| anyhow!("audio client: {e}"))?)
        }
    }

    fn init(kind: AudioSourceKind, format: &WaveFormat) -> Result<Self> {
        let mut client = Self::client(kind)?;
        client
            .initialize_client(
                format,
                &Direction::Capture,
                // 200 ms device buffer: ample slack, engine period stays ~10 ms.
                &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 2_000_000 },
            )
            .map_err(|e| anyhow!("{} init failed: {e}", kind.label()))?;
        let event = client.set_get_eventhandle().map_err(|e| anyhow!("event handle: {e}"))?;
        let capture =
            client.get_audiocaptureclient().map_err(|e| anyhow!("capture client: {e}"))?;
        let block_align = format.get_blockalign() as usize;
        let buffer_frames = client.get_buffer_size().map_err(|e| anyhow!("buffer size: {e}"))?;
        let scratch = vec![0u8; (buffer_frames as usize).max(4096) * block_align];
        Ok(Self { client, event, capture, block_align, scratch, downmix: None, out_float: false })
    }
}

/// Background capture thread feeding [`AudioPacket`]s to a channel.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Result<()>>,
}

impl AudioCapture {
    pub fn start(kind: AudioSourceKind) -> Result<(Self, Receiver<AudioPacket>)> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let stop_flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name(match kind {
                AudioSourceKind::SystemAudio => "clippy-audio".into(),
                AudioSourceKind::Microphone => "clippy-mic".into(),
                AudioSourceKind::Application { .. } => "clippy-audio-app".into(),
            })
            .spawn(move || capture_loop(kind, &stop_flag, &tx))
            .context("failed to spawn audio thread")?;
        Ok((Self { stop, thread }, rx))
    }

    /// True once the capture thread has exited, normally or because the
    /// stream could not be opened or broke. `stop` then reports why.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn stop(self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.join() {
            Ok(result) => result,
            Err(_) => bail!("audio capture thread panicked"),
        }
    }
}

fn capture_loop(kind: AudioSourceKind, stop: &AtomicBool, tx: &Sender<AudioPacket>) -> Result<()> {
    wasapi::initialize_mta().ok().context("COM MTA init failed")?;
    // i16 directly: the audio engine autoconverts its float mix (and a mono
    // or 44.1 kHz microphone), so packets are already in the encoder's format.
    let format = WaveFormat::new(16, 16, &SampleType::Int, SAMPLE_RATE, CHANNELS, None);
    let mut session = WasapiSession::open(kind, &format)?;
    session.client.start_stream().map_err(|e| anyhow!("start stream: {e}"))?;

    while !stop.load(Ordering::Relaxed) {
        if session.event.wait_for_event(100).is_err() {
            continue; // system-wide silence: nothing rendering, keep waiting
        }
        while let Ok(Some(frames)) = session.capture.get_next_packet_size() {
            if frames == 0 {
                break;
            }
            let (read_frames, info) = session
                .capture
                .read_from_device(&mut session.scratch)
                .map_err(|e| anyhow!("read from device: {e}"))?;
            if read_frames == 0 {
                break;
            }
            let packet = AudioPacket {
                qpc_100ns: info.timestamp as i64,
                data: session.samples(read_frames as usize),
            };
            if tx.send(packet).is_err() {
                return Ok(()); // consumer gone — recording ended
            }
        }
    }
    session.client.stop_stream().map_err(|e| anyhow!("stop stream: {e}"))?;
    Ok(())
}

/// Captures `seconds` of system loopback audio and writes it as a
/// 32-bit-float WAV, printing capture-quality statistics (probe mode).
pub fn record_wav(seconds: u64, path: &Path) -> Result<()> {
    wasapi::initialize_mta().ok().context("COM MTA init failed")?;
    println!("capturing {seconds} s of system audio — play some sound to get a signal");

    // f32 stereo 48 kHz with autoconvert: the engine resamples whatever the
    // device mix format is, so downstream code sees exactly one format.
    let format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let mut session = WasapiSession::open(AudioSourceKind::SystemAudio, &format)?;
    let block_align = format.get_blockalign() as usize;
    let mut pcm: Vec<u8> = Vec::with_capacity(SAMPLE_RATE * block_align * seconds as usize);

    let mut packets = 0u64;
    let mut silent_packets = 0u64;
    let mut discontinuities = 0u64;
    let mut first_qpc_100ns: Option<u64> = None;
    let mut last_qpc_100ns = 0u64;

    session.client.start_stream().map_err(|e| anyhow!("start stream: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(seconds);

    while Instant::now() < deadline {
        // Loopback delivers packets only while something renders; timing out
        // just means system-wide silence, so keep waiting until the deadline.
        if session.event.wait_for_event(200).is_err() {
            continue;
        }
        while let Ok(Some(frames)) = session.capture.get_next_packet_size() {
            if frames == 0 {
                break;
            }
            let (read_frames, info) = session
                .capture
                .read_from_device(&mut session.scratch)
                .map_err(|e| anyhow!("read from device: {e}"))?;
            if read_frames == 0 {
                break;
            }
            packets += 1;
            if info.flags.silent {
                silent_packets += 1;
            }
            if info.flags.data_discontinuity && packets > 1 {
                discontinuities += 1;
            }
            if first_qpc_100ns.is_none() {
                first_qpc_100ns = Some(info.timestamp);
            }
            last_qpc_100ns = info.timestamp;
            pcm.extend_from_slice(&session.samples(read_frames as usize));
        }
    }
    session.client.stop_stream().map_err(|e| anyhow!("stop stream: {e}"))?;

    if pcm.is_empty() {
        bail!("no audio packets captured — was anything playing during the test?");
    }
    write_wav_f32(path, &pcm)?;

    let (peak, rms) = analyze_f32(&pcm);
    let captured_secs = pcm.len() as f64 / (SAMPLE_RATE * block_align) as f64;
    let qpc_span_secs =
        first_qpc_100ns.map(|f| (last_qpc_100ns - f) as f64 / 10_000_000.0).unwrap_or(0.0);

    println!("\n[audio capture]");
    println!("  written:         {} ({:.2} s of PCM)", path.display(), captured_secs);
    println!("  packets:         {packets} ({silent_packets} silent)");
    println!("  discontinuities: {discontinuities}");
    println!("  qpc span:        {qpc_span_secs:.2} s");
    println!("  peak:            {:.1} dBFS", 20.0 * peak.max(1e-9).log10());
    println!("  rms:             {:.1} dBFS", 20.0 * rms.max(1e-9).log10());
    if discontinuities > 0 {
        println!("  WARNING: gaps detected — buffer overrun or device stall");
    }
    Ok(())
}

fn analyze_f32(pcm: &[u8]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let samples = pcm.len() / 4;
    for chunk in pcm.chunks_exact(4) {
        let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        peak = peak.max(s.abs());
        sum_sq += f64::from(s) * f64::from(s);
    }
    (peak, (sum_sq / samples.max(1) as f64).sqrt() as f32)
}

/// Minimal RIFF/WAVE writer for 32-bit IEEE-float stereo PCM.
fn write_wav_f32(path: &Path, pcm: &[u8]) -> Result<()> {
    let byte_rate = (SAMPLE_RATE * CHANNELS * 4) as u32;
    let block_align = (CHANNELS * 4) as u16;
    let data_len = pcm.len() as u32;

    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVE")?;
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    file.write_all(&(CHANNELS as u16).to_le_bytes())?;
    file.write_all(&(SAMPLE_RATE as u32).to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    file.write_all(pcm)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_source_kind_names_itself_for_the_log() {
        // These strings reach the user in a warning when a source cannot be
        // opened ("microphone unavailable, …"), so they are user-facing copy,
        // not debug output.
        assert_eq!(AudioSourceKind::SystemAudio.label(), "system audio");
        assert_eq!(AudioSourceKind::Microphone.label(), "microphone");
        assert_eq!(AudioSourceKind::Application { pid: 4 }.label(), "application audio");
    }

    /// Process loopback needs only a live process id. This process has one, so
    /// the stream must open and start even with nothing playing; on a machine
    /// too old for the feature this is where it would show.
    #[test]
    fn an_application_stream_opens_for_a_live_process() {
        let kind = AudioSourceKind::Application { pid: std::process::id() };
        let (capture, _rx) = AudioCapture::start(kind).expect("spawns");
        std::thread::sleep(Duration::from_millis(400));
        assert!(!capture.is_finished(), "the stream ended early");
        capture.stop().expect("opened, ran and stopped cleanly");
    }

    #[test]
    fn the_two_source_kinds_read_different_devices() {
        // The whole difference between recording the speakers and recording
        // the microphone is which default device is enumerated. If these ever
        // matched, Clippy would silently record system audio twice.
        //
        // `matches!` rather than `assert_ne!`: `wasapi::Direction` is a
        // third-party enum and is not guaranteed to implement `PartialEq`.
        assert!(matches!(AudioSourceKind::SystemAudio.direction(), Direction::Render));
        assert!(matches!(AudioSourceKind::Microphone.direction(), Direction::Capture));
    }
}
