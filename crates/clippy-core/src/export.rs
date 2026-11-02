//! Fast-mode clip export: a keyframe-snapped stream copy of an existing MP4.
//!
//! Spec §6.3's `fast` mode. No decoder and no encoder is involved: an
//! `IMFSourceReader` hands over the source's already-encoded H.264 and AAC
//! samples and an `IMFSinkWriter` writes them straight back out. That is what
//! makes an export sub-second and lossless, and it is also why the in-point can
//! only land on a keyframe -- nothing in this pipeline could synthesize the
//! frames between one keyframe and the next.

use std::path::Path;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportStage {
    Indexing,
    Decoding,
    Mixing,
    Writing,
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    pub stage: ExportStage,
    pub percent: u8,
}

/// Cooperative control. Native Media Foundation calls finish before cancellation
/// is observed; progress measures media processed, never an estimated duration.
pub struct ExportControl {
    cancelled: AtomicBool,
    progress: Mutex<ExportProgress>,
    #[cfg(test)]
    cancel_at: std::sync::atomic::AtomicU8,
}

impl Default for ExportControl {
    fn default() -> Self {
        Self::new()
    }
}
impl ExportControl {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(ExportProgress { stage: ExportStage::Indexing, percent: 0 }),
            #[cfg(test)]
            cancel_at: std::sync::atomic::AtomicU8::new(255),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn progress(&self) -> ExportProgress {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(ExportCancelled.into());
        }
        Ok(())
    }
    fn report(&self, stage: ExportStage, percent: u8) -> Result<()> {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if stage >= progress.stage && percent >= progress.percent {
            *progress = ExportProgress { stage, percent: percent.min(99) };
        }
        #[cfg(test)]
        if progress.percent >= self.cancel_at.load(Ordering::Relaxed) {
            self.cancel();
        }
        drop(progress);
        self.check()
    }
}

#[derive(Debug)]
pub struct ExportCancelled;
impl std::fmt::Display for ExportCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("export annulé")
    }
}
impl std::error::Error for ExportCancelled {}

fn media_percent(position: i64, start: i64, end: i64, low: u8, high: u8) -> u8 {
    let span = end.saturating_sub(start).max(1) as u128;
    let done = position.saturating_sub(start).max(0) as u128;
    low + ((done.min(span) * u128::from(high - low)) / span) as u8
}

use anyhow::{Context as _, Result};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader,
    MF_E_INVALIDSTREAMNUMBER, MF_MT_MAJOR_TYPE, MF_READWRITE_DISABLE_CONVERTERS,
    MF_SINK_WRITER_DISABLE_THROTTLING, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MFCreateAttributes,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio,
    MFSampleExtension_CleanPoint,
};
use windows::core::HSTRING;

use crate::encode::mf::ensure_mf_started;

mod precise;
pub use precise::{
    FrameIndex, PreciseExport, SharePlan, export_precise, export_precise_with_control, frame_index,
    share_plan, vertical_crop,
};

/// Shortest export worth writing. Below this the output is a file with one or
/// two frames in it, which is a mistake rather than a clip.
pub const MIN_TRIM_MS: u64 = 200;

/// Latest keyframe at or before `start_ms`.
///
/// Snapping *backwards* rather than to the nearest keyframe is deliberate: a
/// forward snap silently drops footage the user explicitly asked to keep, and
/// the frame they set the in-point on is usually the one that matters.
pub fn snap_start(keyframes_ms: &[u64], start_ms: u64) -> u64 {
    keyframes_ms.iter().copied().rfind(|&k| k <= start_ms).unwrap_or(0)
}

/// Orders and clamps a requested range against the clip's real duration.
///
/// `Err` is the text of the error response, so it says what was wrong in terms
/// the person driving the socket can act on.
pub fn clamp_range(duration_ms: u64, start_ms: u64, end_ms: u64) -> Result<(u64, u64), String> {
    if duration_ms == 0 {
        return Err("this clip has no duration to trim".into());
    }
    if end_ms <= start_ms {
        return Err(format!("end_ms ({end_ms}) must be greater than start_ms ({start_ms})"));
    }
    let start = start_ms.min(duration_ms);
    let end = end_ms.min(duration_ms);
    if end.saturating_sub(start) < MIN_TRIM_MS {
        return Err(format!("a trim must be at least {MIN_TRIM_MS} ms long"));
    }
    Ok((start, end))
}

/// A read that returns no sample without being the end of the stream is legal
/// (a format change, a gap). Skipping is correct; treating it as the end would
/// truncate. Bounded so a source that only ever returns nothing cannot spin
/// forever -- the same reasoning as `sound/decode.rs`'s
/// `MAX_CONSECUTIVE_EMPTY_READS`, but a smaller number. `to_wav`'s loop runs
/// while the daemon's config mutex is held, so a hang there wedges every
/// `status` and `config.get` call, and the clip path too -- that severity is
/// why it is set generous, at 1,000. This loop holds no lock and blocks
/// nothing but its own caller, so a tighter bound is deliberate here.
const MAX_CONSECUTIVE_EMPTY_READS: u32 = 64;

/// Advances the consecutive-empty-read counter, bailing with `source` and
/// `label` named once [`MAX_CONSECUTIVE_EMPTY_READS`] is exceeded. `label`
/// names which stream was being read ("video" or "audio"), since both
/// `keyframes_ms`'s video-only pass and `copy_stream`'s pass over either
/// track call this same function.
///
/// Pulled out of the read loop so this bound can be unit-tested without
/// Media Foundation, the same way -- and for the same reason -- as
/// `sound/decode.rs`'s `count_empty_read`: every read loop in this file
/// calls this exact function rather than repeating the check inline.
fn count_empty_read(empty_reads: u32, source: &Path, label: &str) -> Result<u32> {
    let empty_reads = empty_reads + 1;
    if empty_reads > MAX_CONSECUTIVE_EMPTY_READS {
        anyhow::bail!(
            "{} a cessé de fournir des échantillons {label} après {MAX_CONSECUTIVE_EMPTY_READS} \
             lectures vides consécutives",
            source.display()
        );
    }
    Ok(empty_reads)
}

fn video_stream() -> u32 {
    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32
}

/// Opens `source` for **compressed** reading.
///
/// The load-bearing detail is what this does *not* do: it never calls
/// `SetCurrentMediaType`. Ask a source reader for a decoded type and it inserts
/// a decoder; leave the type alone and it hands over the file's own H.264 and
/// AAC samples untouched. That is the whole stream copy.
fn open_compressed(source: &Path) -> Result<IMFSourceReader> {
    if !source.is_file() {
        anyhow::bail!("{} n’est pas un fichier", source.display());
    }
    ensure_mf_started()?;
    let reader: IMFSourceReader =
        unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(source), None) }
            .with_context(|| format!("Windows n’a pas pu ouvrir {}", source.display()))?;
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .context("deselecting every stream")?;
        reader.SetStreamSelection(video_stream(), true).context("selecting the video stream")?;
    }
    Ok(reader)
}

/// One compressed sample's timing, without the payload.
struct SampleTiming {
    pts_100ns: i64,
    // `keyframes_ms` only needs the position, not the length. Task 2's remux
    // reads this same field to reproduce each sample's duration in the copy,
    // so it stays on the struct rather than being trimmed to what this file
    // alone uses.
    #[allow(dead_code)]
    duration_100ns: i64,
    keyframe: bool,
}

fn timing(sample: &IMFSample) -> SampleTiming {
    unsafe {
        SampleTiming {
            pts_100ns: sample.GetSampleTime().unwrap_or(0),
            duration_100ns: sample.GetSampleDuration().unwrap_or(0),
            // Absent means "not a clean point". An error here is the attribute
            // being missing, which is the common case for a P-frame.
            keyframe: sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1,
        }
    }
}

/// Every keyframe position in `source`, in milliseconds, ascending.
///
/// Always starts with 0: the first sample of a Clippy clip is a keyframe by
/// construction (`replay.rs` refuses to mux a snapshot whose first packet is
/// not one), and a UI that draws ticks needs a tick at the start regardless.
///
/// This is a full demux pass with no decoding, which on a replay-buffer-length
/// clip is tens of milliseconds. It is not cached: the UI asks once per clip
/// opened, and a cache would have to be invalidated on delete and on a clip_dir
/// move for no measurable gain.
pub fn keyframes_ms(source: &Path) -> Result<Vec<u64>> {
    keyframes_with_control(source, &ExportControl::new())
}

fn keyframes_with_control(source: &Path, control: &ExportControl) -> Result<Vec<u64>> {
    control.check()?;
    let reader = open_compressed(source)?;
    let stream = video_stream();
    let mut keys = Vec::new();
    let mut empty_reads = 0u32;
    loop {
        control.check()?;
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)) }
            .with_context(|| format!("la lecture de la vidéo de {} a échoué", source.display()))?;
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            break;
        }
        let Some(sample) = sample else {
            empty_reads = count_empty_read(empty_reads, source, "video")?;
            continue;
        };
        empty_reads = 0;
        let t = timing(&sample);
        if t.keyframe {
            keys.push((t.pts_100ns.max(0) / 10_000) as u64);
        }
    }
    if keys.first() != Some(&0) {
        keys.insert(0, 0);
    }
    control.report(ExportStage::Indexing, 10)?;
    Ok(keys)
}

/// What to export, and to where.
pub struct FastExport<'a> {
    pub source: &'a Path,
    pub dest: &'a Path,
    /// Snapped back to the previous keyframe before anything is written.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// What actually landed, for the caller's `ClipMeta`.
#[derive(Debug)]
pub struct ExportOutcome {
    /// Measured from the samples written, not from the request: the in-point
    /// snapped backwards, so the output is usually a little longer than asked.
    pub duration_ms: u64,
    pub video_packets: usize,
    pub has_audio: bool,
    /// Picture size of the written video when the export resized it; `None`
    /// means the source's own size.
    pub frame_size: Option<(u32, u32)>,
}

/// True when `dest` and `source` are two spellings of one file.
///
/// A plain `==` only catches the identical spelling, and
/// `MFCreateSinkWriterFromURL` truncates on call: `clips\a.mp4` against
/// `clips\sub\..\a.mp4`, or against the same path in different case, would
/// destroy the very clip being exported. `canonicalize` resolves all of those,
/// but it opens the file to do it, so it fails on a `dest` that does not exist
/// yet -- which is the ordinary case, and a path that cannot be opened at all
/// is not the source clip. Hence the plain comparison first and the canonical
/// one only as a second opinion.
fn names_the_same_file(dest: &Path, source: &Path) -> bool {
    if dest == source {
        return true;
    }
    match (dest.canonicalize(), source.canonicalize()) {
        (Ok(dest), Ok(source)) => dest == source,
        _ => false,
    }
}

/// How far an attempt got before it failed, determining whether `dest` is
/// this export's partial output to clean up.
enum Failed {
    /// Failed before `MFCreateSinkWriterFromURL` was called, so `dest` was
    /// never created and never truncated. Anything sitting at that path
    /// belongs to whoever put it there, and deleting it on the way out of a
    /// failure would be destroying a stranger's file.
    BeforeCreating(anyhow::Error),
    /// Failed at or after the call that creates `dest`. Whatever is there now
    /// is this attempt's own half-written output -- truncated, missing its
    /// `moov` atom -- and Task 4 hands exports straight to `library::scan`,
    /// which adopts any bare .mp4 it finds as a real clip. It has to go.
    AfterCreating(anyhow::Error),
}

/// One attempt's result. Named because `Result` in this module is
/// `anyhow::Result`, and an attempt's error deliberately is not an
/// `anyhow::Error`: it carries where the failure landed as well as what it
/// was.
type Attempt = std::result::Result<ExportOutcome, Failed>;

/// Stream-copies `start_ms..end_ms` of `source` into a new MP4 at `dest`.
///
/// The in-point snaps back to the previous keyframe (spec 6.3). The out-point
/// does not need snapping: cutting after a P-frame is fine, every frame in the
/// output still has its reference.
///
/// Every source audio stream is retained in its original order. Audio failures
/// fail the export explicitly; a requested clip must never lose its sound.
pub fn export_fast(request: FastExport<'_>) -> Result<ExportOutcome> {
    export_fast_with_control(request, &ExportControl::new())
}

pub fn export_fast_with_control(
    request: FastExport<'_>,
    control: &ExportControl,
) -> Result<ExportOutcome> {
    control.check()?;
    // `MFCreateSinkWriterFromURL` truncates whatever `dest` names the moment
    // it is called. Nothing downstream of this function would notice or refuse
    // an export written over the clip it is reading -- this is the one place
    // that must.
    if names_the_same_file(request.dest, request.source) {
        anyhow::bail!("la destination de l’export ne peut pas être le clip source");
    }

    // Deliberately before any attempt: a source that cannot even be indexed
    // must fail with `dest` untouched, rather than after a sink writer has
    // already truncated it.
    let keys = keyframes_with_control(request.source, control)?;
    control.report(ExportStage::Writing, 50)?;
    let start_ms = snap_start(&keys, request.start_ms);
    let start_100ns = (start_ms as i64) * 10_000;
    let end_100ns = (request.end_ms as i64) * 10_000;

    finish_attempt(request.dest, || {
        attempt_export(request.source, request.dest, start_100ns, end_100ns, control)
    })
}

/// Clean up only output created by this attempt, preserving the original
/// error. In particular, an audio failure cannot turn into a silent success.
fn finish_attempt(dest: &Path, attempt: impl FnOnce() -> Attempt) -> Result<ExportOutcome> {
    match attempt() {
        Ok(outcome) => Ok(outcome),
        Err(Failed::BeforeCreating(err)) => Err(err),
        Err(Failed::AfterCreating(err)) => {
            discard(dest);
            Err(err)
        }
    }
}

/// Deletes a half-written `dest`, and says so in the log if it cannot.
///
/// The delete is best-effort by necessity — the export has already failed and
/// nothing better is going to happen by failing harder — but *silently*
/// best-effort was the wrong shape. Windows Defender routinely still holds a
/// handle on a file it has just finished scanning, so `remove_file` comes back
/// with a sharing violation, and a discarded error left the truncated MP4 on
/// disk with nothing anywhere explaining where it came from. (`library::scan`
/// now skips zero-byte `.mp4`s, which covers the commonest shape of leftover;
/// this covers the rest, and turns either into something diagnosable.)
///
/// `NotFound` is not a failure: it is the state this function exists to reach.
fn discard(dest: &Path) {
    match std::fs::remove_file(dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            path = %dest.display(),
            error = %e,
            "could not delete a failed export; a broken .mp4 may be left behind"
        ),
    }
}

/// One whole export attempt, from opening the source through `Finalize`.
///
fn attempt_export(
    source: &Path,
    dest: &Path,
    start_100ns: i64,
    end_100ns: i64,
    control: &ExportControl,
) -> Attempt {
    let (reader, video_type, audio_types) = prepare(source).map_err(Failed::BeforeCreating)?;
    control.check().map_err(Failed::BeforeCreating)?;
    run_export(&reader, &video_type, &audio_types, dest, start_100ns, end_100ns, source, control)
        .map_err(Failed::AfterCreating)
}

/// Opens `source` for compressed reading and collects the media types the
/// export will declare.
///
/// Every step is on the source side. Nothing here touches `dest`, which is
/// what lets [`attempt_export`] label a failure from this function as "nothing
/// landed".
fn prepare(source: &Path) -> Result<(IMFSourceReader, IMFMediaType, Vec<(u32, IMFMediaType)>)> {
    let reader = open_compressed(source)?;
    let video_type = unsafe { reader.GetNativeMediaType(video_stream(), 0) }
        .context("ce clip n’a pas de piste vidéo")?;
    let mut audio_types = Vec::new();
    for stream in audio_stream_indices(&reader)? {
        unsafe { reader.SetStreamSelection(stream, true) }
            .with_context(|| format!("sélection de la piste audio {stream}"))?;
        let native = unsafe { reader.GetNativeMediaType(stream, 0) }
            .with_context(|| format!("lecture du format audio {stream}"))?;
        audio_types.push((stream, native));
    }
    Ok((reader, video_type, audio_types))
}

/// Everything from creating the sink writer through `Finalize`. Split out of
/// [`attempt_export`] so that every failure from the moment `dest` is created
/// -- the create call itself included, since it truncates `dest` and can still
/// fail after that -- is one `Err` with one meaning. This owns the writer by
/// value, so any early return, `?` on a mid-copy failure included, drops it,
/// and the file handle it holds, before the caller deletes the file.
#[allow(clippy::too_many_arguments)]
fn run_export(
    reader: &IMFSourceReader,
    video_type: &IMFMediaType,
    audio_types: &[(u32, IMFMediaType)],
    dest: &Path,
    start_100ns: i64,
    end_100ns: i64,
    source: &Path,
    control: &ExportControl,
) -> Result<ExportOutcome> {
    let (writer, video_out, audio_out) = open_sink(dest, video_type, audio_types)?;

    let video = copy_stream_controlled(
        reader,
        &writer,
        video_stream(),
        video_out,
        start_100ns,
        end_100ns,
        source,
        "video",
        control,
        50,
        70,
    )?;

    if video.packets == 0 {
        // An empty output is worse than no output: it lands in the library as
        // a clip that will not play. Checked here rather than after
        // `Finalize`, because a sink that has been handed no samples at all
        // fails `Finalize` with MF_E_SINK_NO_SAMPLES_PROCESSED (0xC00D4A44)
        // -- measured against this codepath, not assumed -- and the caller
        // deserves this sentence rather than that HRESULT. Video is copied
        // first, so zero packets here means the sink has seen nothing.
        anyhow::bail!("cette plage ne contient aucune vidéo");
    }

    let has_audio = copy_audio_streams_controlled(
        reader,
        &writer,
        audio_types,
        &audio_out,
        start_100ns,
        end_100ns,
        source,
        control,
    )?;

    control.report(ExportStage::Finalizing, 95)?;
    unsafe { writer.Finalize().context("Finalize(export)")? };
    control.report(ExportStage::Finalizing, 99)?;

    Ok(ExportOutcome {
        duration_ms: (video.written_100ns / 10_000).max(0) as u64,
        video_packets: video.packets,
        has_audio,
        frame_size: None,
    })
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn copy_audio_streams(
    reader: &IMFSourceReader,
    writer: &IMFSinkWriter,
    audio_types: &[(u32, IMFMediaType)],
    audio_out: &[u32],
    start_100ns: i64,
    end_100ns: i64,
    source: &Path,
) -> Result<bool> {
    copy_audio_streams_controlled(
        reader,
        writer,
        audio_types,
        audio_out,
        start_100ns,
        end_100ns,
        source,
        &ExportControl::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn copy_audio_streams_controlled(
    reader: &IMFSourceReader,
    writer: &IMFSinkWriter,
    audio_types: &[(u32, IMFMediaType)],
    audio_out: &[u32],
    start_100ns: i64,
    end_100ns: i64,
    source: &Path,
    control: &ExportControl,
) -> Result<bool> {
    let mut has_audio = false;
    for (index, ((input, _), output)) in
        audio_types.iter().zip(audio_out.iter().copied()).enumerate()
    {
        let low = 70 + (index * 25 / audio_types.len().max(1)) as u8;
        let high = 70 + ((index + 1) * 25 / audio_types.len().max(1)) as u8;
        let audio = copy_stream_controlled(
            reader,
            writer,
            *input,
            output,
            start_100ns,
            end_100ns,
            source,
            "audio",
            control,
            low,
            high,
        )?;
        // An empty declared stream may be omitted by the MP4 sink, shifting
        // every following ordinal. Refuse instead of writing false metadata.
        if audio.packets == 0 {
            anyhow::bail!("la piste audio {input} ne contient aucun échantillon dans cette plage");
        }
        has_audio = true;
    }

    Ok(has_audio)
}

fn add_audio_passthrough(
    writer: &IMFSinkWriter,
    audio_types: &[(u32, IMFMediaType)],
) -> Result<Vec<u32>> {
    let mut audio_out = Vec::new();
    for (_, audio_type) in audio_types {
        unsafe {
            let stream = writer.AddStream(audio_type).context("AddStream(export audio)")?;
            writer
                .SetInputMediaType(stream, audio_type, None)
                .context("SetInputMediaType(export audio passthrough)")?;
            audio_out.push(stream);
        }
    }
    Ok(audio_out)
}

/// Creates a passthrough writer with video followed by every audio track in
/// file order. Converters and throttling are disabled: this copies compressed
/// samples without introducing an encoder or waiting for lagging streams.
fn open_sink(
    dest: &Path,
    video_type: &IMFMediaType,
    audio_types: &[(u32, IMFMediaType)],
) -> Result<(IMFSinkWriter, u32, Vec<u32>)> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        // The passthrough guarantee, enforced rather than assumed. See above.
        attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dest), None, Some(&attrs))
            .with_context(|| format!("impossible de créer {}", dest.display()))?;

        let video_out = writer.AddStream(video_type).context("AddStream(export video)")?;
        writer
            .SetInputMediaType(video_out, video_type, None)
            .context("SetInputMediaType(export video passthrough)")?;

        let audio_out = add_audio_passthrough(&writer, audio_types)?;

        writer.BeginWriting().context("BeginWriting(export)")?;
        Ok((writer, video_out, audio_out))
    }
}

struct CopiedStream {
    packets: usize,
    written_100ns: i64,
}

/// Reads one stream start-to-finish, writing only the samples inside the range
/// and rebasing their timestamps so the output starts at zero. `label`
/// ("video" or "audio") is only for `count_empty_read`'s error message.
#[allow(clippy::too_many_arguments)]
fn copy_stream(
    reader: &IMFSourceReader,
    writer: &IMFSinkWriter,
    in_stream: u32,
    out_stream: u32,
    start_100ns: i64,
    end_100ns: i64,
    source: &Path,
    label: &str,
) -> Result<CopiedStream> {
    copy_stream_controlled(
        reader,
        writer,
        in_stream,
        out_stream,
        start_100ns,
        end_100ns,
        source,
        label,
        &ExportControl::new(),
        50,
        95,
    )
}

#[allow(clippy::too_many_arguments)]
fn copy_stream_controlled(
    reader: &IMFSourceReader,
    writer: &IMFSinkWriter,
    in_stream: u32,
    out_stream: u32,
    start_100ns: i64,
    end_100ns: i64,
    source: &Path,
    label: &str,
    control: &ExportControl,
    low: u8,
    high: u8,
) -> Result<CopiedStream> {
    let mut packets = 0usize;
    let mut written_100ns = 0i64;
    let mut empty_reads = 0u32;
    loop {
        control.check()?;
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        unsafe { reader.ReadSample(in_stream, 0, None, Some(&mut flags), None, Some(&mut sample)) }
            .with_context(|| format!("la lecture de {} a échoué", source.display()))?;
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            break;
        }
        let Some(sample) = sample else {
            empty_reads = count_empty_read(empty_reads, source, label)?;
            continue;
        };
        empty_reads = 0;
        let t = timing(&sample);
        if t.pts_100ns < start_100ns {
            continue;
        }
        if t.pts_100ns >= end_100ns {
            break;
        }
        unsafe {
            sample.SetSampleTime(t.pts_100ns - start_100ns).context("rebasing the timestamp")?;
            writer.WriteSample(out_stream, &sample).context("WriteSample(export)")?;
        }
        packets += 1;
        written_100ns = (t.pts_100ns - start_100ns) + t.duration_100ns;
        control.report(
            ExportStage::Writing,
            media_percent(t.pts_100ns, start_100ns, end_100ns, low, high),
        )?;
    }
    Ok(CopiedStream { packets, written_100ns })
}

// ---------------------------------------------------------------------------
// Separate audio tracks
//
// A multi-track clip carries one AAC stream per source (see `replay::write_clip`):
// stream 0 is the pre-mixed "Mix" and the rest are the sources on their own.
// "Ordinal" below is a track's position among the *file's audio streams* in
// that order, which is the index `ClipMeta::audio_tracks` and the editor use.
// ---------------------------------------------------------------------------

/// The source reader's stream indexes of the file's audio streams, in file order.
///
/// The reader numbers an MP4's audio streams last-written first: a clip written
/// with its tracks as Mix, game, chat, microphone comes back as microphone,
/// chat, game, Mix. Our writer always writes the same way, so reversing that
/// listing gives the order the file (and the sidecar's names) use.
/// `each_track_comes_back_as_the_stream_written_as_it` holds this to account.
fn audio_stream_indices(reader: &IMFSourceReader) -> Result<Vec<u32>> {
    let mut found = Vec::new();
    // A file never has anywhere near this many streams; the bound only keeps a
    // misbehaving source from looping forever.
    for index in 0u32..64 {
        let native = match unsafe { reader.GetNativeMediaType(index, 0) } {
            Ok(native) => native,
            Err(error) if error.code() == MF_E_INVALIDSTREAMNUMBER => {
                found.reverse();
                return Ok(found);
            }
            Err(error) => return Err(error).context("énumération des pistes audio"),
        };
        if unsafe { native.GetGUID(&MF_MT_MAJOR_TYPE) }.context("lecture du type de piste")?
            == MFMediaType_Audio
        {
            found.push(index);
        }
    }
    anyhow::bail!("ce fichier dépasse la limite de 64 flux média")
}

fn open_reader(source: &Path) -> Result<IMFSourceReader> {
    if !source.is_file() {
        anyhow::bail!("{} n’est pas un fichier", source.display());
    }
    ensure_mf_started()?;
    unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(source), None) }
        .with_context(|| format!("Windows n’a pas pu ouvrir {}", source.display()))
}

/// How many audio streams the file at `source` has.
pub fn audio_track_count(source: &Path) -> Result<usize> {
    Ok(audio_stream_indices(&open_reader(source)?)?.len())
}

/// What an MP4 says about itself: enough to list a clip the library has no
/// record of (dropped in by hand, or its record lost). Reads the headers only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Probe {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio_tracks: usize,
}

pub fn probe(source: &Path) -> Result<Probe> {
    use windows::Win32::Media::MediaFoundation::{
        MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_PD_DURATION, MF_SOURCE_READER_MEDIASOURCE,
    };
    let reader = open_reader(source)?;
    let duration = unsafe {
        reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
    }
    .context("durée du fichier")?;
    let duration_100ns = u64::try_from(&duration).unwrap_or(0);
    let mut found = Probe { duration_ms: duration_100ns / 10_000, ..Probe::default() };
    if let Ok(video) = unsafe { reader.GetNativeMediaType(video_stream(), 0) } {
        let split = |packed: u64| ((packed >> 32) as u32, packed as u32);
        if let Ok(size) = unsafe { video.GetUINT64(&MF_MT_FRAME_SIZE) } {
            (found.width, found.height) = split(size);
        }
        if let Ok(rate) = unsafe { video.GetUINT64(&MF_MT_FRAME_RATE) } {
            let (num, den) = split(rate);
            found.fps = if den == 0 { 0 } else { (f64::from(num) / f64::from(den)).round() as u32 };
        }
    }
    found.audio_tracks = audio_stream_indices(&reader)?.len();
    Ok(found)
}

/// A reader that delivers only `stream`, compressed as stored or -- with
/// `decode` -- decoded to the engine's PCM format.
fn open_single_stream(source: &Path, stream: u32, decode: bool) -> Result<IMFSourceReader> {
    let reader = open_reader(source)?;
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .context("deselecting every stream")?;
        reader.SetStreamSelection(stream, true).context("selecting the audio stream")?;
        if decode {
            let pcm = crate::encode::mf::pcm_audio_type()?;
            reader
                .SetCurrentMediaType(stream, None, &pcm)
                .context("asking the reader to decode the audio to PCM")?;
        }
    }
    Ok(reader)
}

/// Copies one audio track of `source` into an audio-only MP4 at `dest`, without
/// decoding it. The editor plays these next to a muted video, one per track,
/// because a web view cannot select between a file's audio streams itself.
pub fn extract_audio_track(source: &Path, ordinal: usize, dest: &Path) -> Result<()> {
    if names_the_same_file(dest, source) {
        anyhow::bail!("la destination ne peut pas être le clip source");
    }
    let indices = audio_stream_indices(&open_reader(source)?)?;
    let stream = *indices
        .get(ordinal)
        .with_context(|| format!("ce clip n’a pas de piste audio {ordinal}"))?;
    let reader = open_single_stream(source, stream, false)?;
    let native =
        unsafe { reader.GetNativeMediaType(stream, 0) }.context("reading the audio type")?;

    let result = (|| -> Result<()> {
        let (writer, out) = open_audio_only_sink(dest, &native)?;
        let copied = copy_stream(&reader, &writer, stream, out, 0, i64::MAX, source, "audio")?;
        if copied.packets == 0 {
            anyhow::bail!("la piste audio {ordinal} est vide");
        }
        unsafe { writer.Finalize().context("Finalize(audio track)")? };
        Ok(())
    })();
    if result.is_err() {
        discard(dest);
    }
    result
}

fn open_audio_only_sink(dest: &Path, audio_type: &IMFMediaType) -> Result<(IMFSinkWriter, u32)> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dest), None, Some(&attrs))
            .with_context(|| format!("impossible de créer {}", dest.display()))?;
        let stream = writer.AddStream(audio_type).context("AddStream(audio track)")?;
        writer
            .SetInputMediaType(stream, audio_type, None)
            .context("SetInputMediaType(audio track passthrough)")?;
        writer.BeginWriting().context("BeginWriting(audio track)")?;
        Ok((writer, stream))
    }
}

/// One track an export keeps, and how loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackChoice {
    /// Position among the file's audio streams (0 is the pre-mix when the file
    /// has several).
    pub ordinal: usize,
    /// 0..=200, on the same taper the recorder's levels use; 100 is unchanged.
    pub gain_percent: u32,
}

/// Whether to retain editable audio stems alongside the ordinary-player mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemixOutput {
    MixAndStems,
    MixOnly,
}

/// A trimmed export whose audio is rebuilt from the tracks the user chose.
pub struct RemixExport<'a> {
    pub source: &'a Path,
    pub dest: &'a Path,
    /// Snapped back to the previous keyframe, as in [`FastExport`].
    pub start_ms: u64,
    pub end_ms: u64,
    pub tracks: &'a [TrackChoice],
    pub output: RemixOutput,
}

/// Decodes one track to PCM over `start_100ns..end_100ns`, returning it with
/// the time its first sample lands at, relative to `start_100ns`.
#[cfg(test)]
fn decode_track(
    source: &Path,
    ordinal: usize,
    start_100ns: i64,
    end_100ns: i64,
) -> Result<(Vec<u8>, i64)> {
    decode_track_controlled(source, ordinal, start_100ns, end_100ns, &ExportControl::new(), 10, 45)
}

fn decode_track_controlled(
    source: &Path,
    ordinal: usize,
    start_100ns: i64,
    end_100ns: i64,
    control: &ExportControl,
    low: u8,
    high: u8,
) -> Result<(Vec<u8>, i64)> {
    control.check()?;
    let indices = audio_stream_indices(&open_reader(source)?)?;
    let stream = *indices
        .get(ordinal)
        .with_context(|| format!("ce clip n’a pas de piste audio {ordinal}"))?;
    let reader = open_single_stream(source, stream, true)?;
    let mut pcm = Vec::new();
    let mut offset_100ns = 0i64;
    let mut pcm_origin = None;
    let mut empty_reads = 0u32;
    loop {
        control.check()?;
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)) }
            .with_context(|| format!("la lecture de {} a échoué", source.display()))?;
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            break;
        }
        let Some(sample) = sample else {
            empty_reads = count_empty_read(empty_reads, source, "audio")?;
            continue;
        };
        empty_reads = 0;
        let t = timing(&sample);
        let origin = *pcm_origin.get_or_insert(t.pts_100ns);
        if t.pts_100ns + t.duration_100ns <= start_100ns {
            continue;
        }
        if t.pts_100ns >= end_100ns {
            break;
        }
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer().context("reading decoded audio")?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut data, None, Some(&mut len)).context("locking decoded audio")?;
            let bytes = std::slice::from_raw_parts(data, len as usize);
            let (first, last) =
                pcm_frame_range_on_grid(origin, t.pts_100ns, start_100ns, end_100ns, bytes.len());
            if first < last {
                if pcm.is_empty() {
                    offset_100ns = (t.pts_100ns
                        + first as i64 * 10_000_000 / crate::capture::audio::SAMPLE_RATE as i64
                        - start_100ns)
                        .max(0);
                }
                let block = crate::capture::audio::ENCODER_BLOCK_ALIGN;
                pcm.extend_from_slice(&bytes[first * block..last * block]);
            }
            buffer.Unlock().context("unlocking decoded audio")?;
        }
        control.report(
            ExportStage::Decoding,
            media_percent(t.pts_100ns, start_100ns, end_100ns, low, high),
        )?;
    }
    Ok((pcm, offset_100ns))
}

/// Keep only PCM frame starts within the half-open requested interval.
#[cfg(test)]
fn pcm_frame_range(pts: i64, start: i64, end: i64, byte_len: usize) -> (usize, usize) {
    pcm_frame_range_on_grid(pts, pts, start, end, byte_len)
}

fn pcm_frame_range_on_grid(
    origin: i64,
    pts: i64,
    start: i64,
    end: i64,
    byte_len: usize,
) -> (usize, usize) {
    let count = byte_len / crate::capture::audio::ENCODER_BLOCK_ALIGN;
    // Packet timestamps round a rational PCM sample position to 100 ns. Use
    // the track's sample grid so this rounding cannot discard an extra frame.
    let rate = crate::capture::audio::SAMPLE_RATE as i128;
    let packet_frame =
        ((i128::from(pts) - i128::from(origin)).max(0) * rate + 5_000_000) / 10_000_000;
    let at = |bound: i64| {
        let delta = i128::from(bound) - i128::from(origin);
        let frames = (delta.max(0) * rate + 9_999_999) / 10_000_000;
        (frames - packet_frame).clamp(0, count as i128) as usize
    };
    (at(start), at(end))
}

/// Builds the ordinary-player mix from the chosen tracks at their gains,
/// optionally retaining those stems after it for further editing.
#[cfg(test)]
fn remix_streams(
    decoded: Vec<(Vec<u8>, i64)>,
    tracks: &[TrackChoice],
    output: RemixOutput,
) -> Vec<(Vec<u8>, i64)> {
    remix_streams_controlled(decoded, tracks, output, &ExportControl::new())
        .expect("fresh control cannot be cancelled")
}

fn remix_streams_controlled(
    decoded: Vec<(Vec<u8>, i64)>,
    tracks: &[TrackChoice],
    output: RemixOutput,
    control: &ExportControl,
) -> Result<Vec<(Vec<u8>, i64)>> {
    use crate::capture::audio::{apply_pcm_gain, mix_pcm_into, percent_to_gain};
    control.report(ExportStage::Mixing, 45)?;
    let mut stems = Vec::with_capacity(decoded.len());
    let chunk_bytes =
        crate::capture::audio::SAMPLE_RATE * crate::capture::audio::ENCODER_BLOCK_ALIGN;
    for ((mut pcm, offset), choice) in decoded.into_iter().zip(tracks) {
        for chunk in pcm.chunks_mut(chunk_bytes) {
            control.check()?;
            apply_pcm_gain(chunk, percent_to_gain(choice.gain_percent));
        }
        stems.push((pcm, offset));
    }
    if stems.len() > 1 {
        let block = crate::capture::audio::ENCODER_BLOCK_ALIGN;
        let offset = stems.iter().map(|(_, o)| *o).min().unwrap_or(0);
        let frames = |(pcm, o): &(Vec<u8>, i64)| {
            ((o - offset) as usize * crate::capture::audio::SAMPLE_RATE / 10_000_000) * block
                + pcm.len()
        };
        let mut mix = vec![0u8; stems.iter().map(frames).max().unwrap_or(0)];
        for stem in &stems {
            let shift = ((stem.1 - offset) as usize * crate::capture::audio::SAMPLE_RATE
                / 10_000_000)
                * block;
            for (index, chunk) in stem.0.chunks(chunk_bytes).enumerate() {
                control.check()?;
                let at = shift + index * chunk_bytes;
                mix_pcm_into(&mut mix[at..at + chunk.len()], chunk, 1.0);
            }
        }
        if output == RemixOutput::MixOnly {
            control.report(ExportStage::Mixing, 50)?;
            return Ok(vec![(mix, offset)]);
        }
        stems.insert(0, (mix, offset));
    }
    control.report(ExportStage::Mixing, 50)?;
    Ok(stems)
}

/// Stream-copies the video of `start_ms..end_ms` and rebuilds the audio from
/// `tracks`: each decoded and scaled by its gain. A share writes their sum as
/// its only audio stream; a master also retains the individual stems. Muted
/// tracks are simply not listed. The video is untouched, so the in-point still
/// snaps back to a keyframe.
pub fn export_remix(request: RemixExport<'_>) -> Result<ExportOutcome> {
    export_remix_with_control(request, &ExportControl::new())
}

pub fn export_remix_with_control(
    request: RemixExport<'_>,
    control: &ExportControl,
) -> Result<ExportOutcome> {
    control.check()?;
    if names_the_same_file(request.dest, request.source) {
        anyhow::bail!("la destination de l’export ne peut pas être le clip source");
    }
    let keys = keyframes_with_control(request.source, control)?;
    control.report(ExportStage::Decoding, 10)?;
    let start_ms = snap_start(&keys, request.start_ms);
    let start_100ns = (start_ms as i64) * 10_000;
    let end_100ns = (request.end_ms as i64) * 10_000;

    // Everything on the source side first, so a failure here leaves `dest`
    // untouched (the same rule `Failed::BeforeCreating` states).
    let decoded = request
        .tracks
        .iter()
        .enumerate()
        .map(|(index, t)| {
            let count = request.tracks.len().max(1);
            let low = 10 + (index * 35 / count) as u8;
            let high = 10 + ((index + 1) * 35 / count) as u8;
            decode_track_controlled(
                request.source,
                t.ordinal,
                start_100ns,
                end_100ns,
                control,
                low,
                high,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let streams = remix_streams_controlled(decoded, request.tracks, request.output, control)?;
    let video_reader = open_compressed(request.source)?;
    let video_type = unsafe { video_reader.GetNativeMediaType(video_stream(), 0) }
        .context("ce clip n’a pas de piste vidéo")?;

    control.report(ExportStage::Writing, 50)?;
    let result = (|| -> Result<ExportOutcome> {
        let muxer = crate::encode::mf::ClipMuxer::new(request.dest, &video_type, streams.len())?;
        let (writer, video_out) = muxer.video_sink();
        let video = copy_stream_controlled(
            &video_reader,
            writer,
            video_stream(),
            video_out,
            start_100ns,
            end_100ns,
            request.source,
            "video",
            control,
            50,
            70,
        )?;
        if video.packets == 0 {
            anyhow::bail!("cette plage ne contient aucune vidéo");
        }
        let second =
            crate::capture::audio::SAMPLE_RATE * crate::capture::audio::ENCODER_BLOCK_ALIGN;
        let total_chunks: usize = streams.iter().map(|(pcm, _)| pcm.len().div_ceil(second)).sum();
        let mut written_chunks = 0;
        for (track, (pcm, offset)) in streams.iter().enumerate() {
            for (i, chunk) in pcm.chunks(second).enumerate() {
                control.check()?;
                let at = offset + (i as i64) * 10_000_000;
                muxer.write_audio(track, at, chunk)?;
                written_chunks += 1;
                control.report(
                    ExportStage::Writing,
                    70 + (written_chunks * 25 / total_chunks.max(1)) as u8,
                )?;
            }
        }
        control.report(ExportStage::Finalizing, 95)?;
        muxer.finish()?;
        control.report(ExportStage::Finalizing, 99)?;
        Ok(ExportOutcome {
            duration_ms: (video.written_100ns / 10_000).max(0) as u64,
            video_packets: video.packets,
            has_audio: streams.iter().any(|(pcm, _)| !pcm.is_empty()),
            frame_size: None,
        })
    })();
    if result.is_err() {
        discard(request.dest);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_is_thread_safe_monotonic_and_cancel_is_typed() {
        let control = std::sync::Arc::new(ExportControl::default());
        let worker = control.clone();
        std::thread::spawn(move || {
            worker.report(ExportStage::Writing, 80).unwrap();
            worker.report(ExportStage::Decoding, 20).unwrap();
            worker.report(ExportStage::Writing, 60).unwrap();
            worker.cancel();
        })
        .join()
        .unwrap();
        assert_eq!(control.progress(), ExportProgress { stage: ExportStage::Writing, percent: 80 });
        assert!(control.is_cancelled());
        assert!(control.check().unwrap_err().downcast_ref::<ExportCancelled>().is_some());
    }

    #[test]
    fn cancelled_exports_leave_existing_destination_untouched() {
        let dir = scratch_dir("precancel");
        let source = dir.join("missing.mp4");
        let dest = dir.join("existing.mp4");
        std::fs::write(&dest, STUB).unwrap();
        let control = ExportControl::new();
        control.cancel();
        let fast = export_fast_with_control(
            FastExport { source: &source, dest: &dest, start_ms: 0, end_ms: 3000 },
            &control,
        )
        .unwrap_err();
        let remix = export_remix_with_control(
            RemixExport {
                source: &source,
                dest: &dest,
                start_ms: 0,
                end_ms: 3000,
                tracks: &[],
                output: RemixOutput::MixOnly,
            },
            &control,
        )
        .unwrap_err();
        assert!(fast.downcast_ref::<ExportCancelled>().is_some());
        assert!(remix.downcast_ref::<ExportCancelled>().is_some());
        assert_eq!(std::fs::read(&dest).unwrap(), STUB);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_during_native_exports_removes_partial_and_preserves_source() {
        let dir = scratch_dir("cancel-native");
        let source = dir.join("source.mp4");
        synthetic_video(&source);
        let source_bytes = std::fs::read(&source).unwrap();
        for remix in [false, true] {
            let dest = dir.join(if remix { "share.mp4" } else { "master.mp4" });
            let control = ExportControl::new();
            // This hook fires from the real loop after video samples have
            // reached the native sink, without timing-dependent thread sleeps.
            control.cancel_at.store(55, Ordering::Relaxed);
            let result = if remix {
                export_remix_with_control(
                    RemixExport {
                        source: &source,
                        dest: &dest,
                        start_ms: 0,
                        end_ms: 3000,
                        tracks: &[TrackChoice { ordinal: 1, gain_percent: 100 }],
                        output: RemixOutput::MixOnly,
                    },
                    &control,
                )
            } else {
                export_fast_with_control(
                    FastExport { source: &source, dest: &dest, start_ms: 0, end_ms: 3000 },
                    &control,
                )
            };
            assert!(result.unwrap_err().downcast_ref::<ExportCancelled>().is_some());
            assert!(control.progress().percent >= 55);
            assert!(!dest.exists(), "partial output must be removed");
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn share_audio_has_one_mix_with_the_same_samples_as_the_master_mix() {
        let pcm = |sample: i16| sample.to_le_bytes().repeat(8);
        let decoded = vec![(pcm(1000), 500), (pcm(2000), 500)];
        let tracks = [
            TrackChoice { ordinal: 1, gain_percent: 100 },
            TrackChoice { ordinal: 2, gain_percent: 50 },
        ];
        let master = remix_streams(decoded.clone(), &tracks, RemixOutput::MixAndStems);
        let share = remix_streams(decoded, &tracks, RemixOutput::MixOnly);
        assert_eq!(master.len(), 3);
        assert_eq!(share.len(), 1);
        assert_eq!(share[0], master[0]);
        assert_eq!(share[0].1, 500);
        // The mixer uses the recorder's gain taper, not a separate export curve.
        let mut expected = pcm(2000);
        crate::capture::audio::apply_pcm_gain(
            &mut expected,
            crate::capture::audio::percent_to_gain(50),
        );
        crate::capture::audio::mix_pcm_into(&mut expected, &pcm(1000), 1.0);
        assert_eq!(share[0].0, expected);
        assert_eq!(master[1].0, pcm(1000));
    }

    #[test]
    fn share_audio_preserves_single_track_gain_and_intentional_silence() {
        assert!(remix_streams(Vec::new(), &[], RemixOutput::MixOnly).is_empty());
        let pcm = 1000i16.to_le_bytes().repeat(4);
        let tracks = [TrackChoice { ordinal: 0, gain_percent: 0 }];
        let streams = remix_streams(vec![(pcm, 0)], &tracks, RemixOutput::MixOnly);
        assert_eq!(streams, vec![(vec![0; 8], 0)]);
    }

    #[test]
    fn snap_start_goes_back_to_the_previous_keyframe() {
        let keys = [0u64, 1000, 2000, 3000];
        assert_eq!(snap_start(&keys, 2500), 2000, "snaps back, never forward");
        assert_eq!(snap_start(&keys, 2000), 2000, "an exact keyframe stays put");
        assert_eq!(snap_start(&keys, 0), 0);
    }

    /// A clip whose index somehow arrives without a leading 0 must still export
    /// from the top rather than refuse.
    #[test]
    fn snap_start_falls_back_to_zero_when_nothing_is_early_enough() {
        assert_eq!(snap_start(&[5000, 6000], 1000), 0);
        assert_eq!(snap_start(&[], 1000), 0);
    }

    #[test]
    fn clamp_range_refuses_inverted_and_tiny_ranges() {
        assert!(clamp_range(10_000, 5_000, 5_000).is_err(), "zero-length");
        assert!(clamp_range(10_000, 5_000, 4_000).is_err(), "inverted");
        assert!(clamp_range(10_000, 0, MIN_TRIM_MS - 1).is_err(), "below the floor");
        assert!(clamp_range(0, 0, 1_000).is_err(), "a clip with no duration");
    }

    #[test]
    fn clamp_range_trims_an_overlong_end_to_the_clip() {
        assert_eq!(clamp_range(10_000, 8_000, 99_000), Ok((8_000, 10_000)));
    }

    /// The bound from `MAX_CONSECUTIVE_EMPTY_READS`: a long run of legitimate
    /// gaps must not trip it (a real file can have a few), but it must still
    /// end the loop eventually so a source that only ever returns nothing
    /// cannot hang `keyframes_ms` forever. This calls the exact function the
    /// read loop calls, so it exercises the real guard without needing Media
    /// Foundation -- same shape as `sound/decode.rs`'s equivalent test. Uses
    /// the "audio" label (rather than "video", as every other test here
    /// does) so this test also pins that the label actually reaches the
    /// message, not just that a message is produced.
    #[test]
    fn the_empty_read_guard_bails_after_the_limit_but_not_before() {
        let path = Path::new(r"C:\clips\fixture.mp4");

        let mut empty_reads = 0u32;
        for _ in 0..MAX_CONSECUTIVE_EMPTY_READS {
            empty_reads = count_empty_read(empty_reads, path, "audio")
                .expect("must not bail before the limit is exceeded");
        }

        let err = count_empty_read(empty_reads, path, "audio").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("fixture.mp4"), "the error must name the file: {message}");
        assert!(message.contains("audio"), "the error must name which stream stalled: {message}");
        assert!(
            message.contains(&MAX_CONSECUTIVE_EMPTY_READS.to_string()),
            "the error should say how many reads it gave up after: {message}"
        );
    }

    /// A single empty read, or a handful, must not bail -- resetting on a
    /// successful sample is what lets a long clip with occasional legitimate
    /// gaps still index fully. This only checks the counter does not fire
    /// early; the reset itself is a plain assignment in the loop, covered
    /// below.
    #[test]
    fn a_handful_of_empty_reads_does_not_bail() {
        let path = Path::new(r"C:\clips\fixture.mp4");

        let mut empty_reads = 0u32;
        for _ in 0..5 {
            empty_reads = count_empty_read(empty_reads, path, "video")
                .expect("a few gaps in a row must not bail");
        }
        assert_eq!(empty_reads, 5);
    }

    /// `sound/decode.rs`'s equivalent test left its reset-on-success line
    /// uncovered, because a bare `empty_reads = 0` triggered by a real sample
    /// is hard to reach without Media Foundation. This pins the interaction
    /// the reset exists for at the level that *can* be checked without MF:
    /// two gaps, then a reset standing in for the loop's `empty_reads = 0` on
    /// a real sample, then one more gap. If the reset did not actually zero
    /// the counter, this would report 4 rather than the true 1 -- and on a
    /// real file, a bound sized for the gaps after one reset would then fire
    /// too early on a clip with several periodic, legitimate gaps.
    #[test]
    fn a_reset_between_gaps_means_only_the_later_gaps_count() {
        let path = Path::new(r"C:\clips\fixture.mp4");

        let mut empty_reads = 0u32;
        empty_reads = count_empty_read(empty_reads, path, "video").expect("gap 1");
        empty_reads = count_empty_read(empty_reads, path, "video").expect("gap 2");
        assert_eq!(empty_reads, 2);

        // Stands in for the read loop's `empty_reads = 0` on a real sample.
        empty_reads = 0;

        empty_reads = count_empty_read(empty_reads, path, "video").expect("gap after the reset");
        assert_eq!(empty_reads, 1, "the two gaps before the reset must not still be counted");
    }

    /// Recognisable content for a file the export did not write. Every
    /// destination-side assertion below turns on being able to tell "this file
    /// is still the one the test put there" from "this file is something the
    /// export produced" -- and, when the path is gone, on knowing that the
    /// only thing that could have removed it is a delete.
    const STUB: &[u8] = b"not a clip -- seeded by the test";

    /// A private directory under the system temp dir, per test. Nothing here
    /// ever reads or writes the developer's real config or clip library.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("clippy-export-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating the scratch dir");
        dir
    }

    fn an_outcome(has_audio: bool) -> ExportOutcome {
        ExportOutcome { duration_ms: 2_000, video_packets: 120, has_audio, frame_size: None }
    }

    #[test]
    fn an_audio_failure_is_reported_and_its_partial_output_removed() {
        let dir = scratch_dir("audio-fail");
        let dest = dir.join("out.mp4");
        let err = finish_attempt(&dest, || {
            std::fs::write(&dest, b"partial video and audio").unwrap();
            Err(Failed::AfterCreating(anyhow::anyhow!("audio stream 2 failed")))
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("audio stream 2 failed"));
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_before_the_sink_exists_leaves_existing_files_untouched() {
        let dir = scratch_dir("before-creating");
        let dest = dir.join("out.mp4");
        std::fs::write(&dest, STUB).unwrap();
        let err = finish_attempt(&dest, || {
            Err(Failed::BeforeCreating(anyhow::anyhow!("source unreadable")))
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("source unreadable"));
        assert_eq!(std::fs::read(&dest).unwrap(), STUB);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_successful_export_keeps_its_output_and_audio() {
        let dir = scratch_dir("success");
        let dest = dir.join("out.mp4");
        let outcome = finish_attempt(&dest, || {
            std::fs::write(&dest, b"complete export").unwrap();
            Ok(an_outcome(true))
        })
        .unwrap();
        assert!(outcome.has_audio);
        assert_eq!(std::fs::read(&dest).unwrap(), b"complete export");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `dest == source` compared as written misses a second spelling of the
    /// same file, and the cost of missing it is the source clip truncated by
    /// `MFCreateSinkWriterFromURL` -- the export destroying the thing it was
    /// asked to copy. Needs no Media Foundation: the refusal happens before
    /// anything is opened.
    #[test]
    fn an_export_onto_the_source_is_refused_however_the_path_is_spelled() {
        let dir = scratch_dir("same-file");
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, STUB).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        // The same file by a different route. `PathBuf` comparison says these
        // are two paths; the filesystem says they are one file.
        let spelled_differently = dir.join("sub").join("..").join("clip.mp4");
        assert_ne!(clip, spelled_differently, "the test needs two spellings, not two paths");

        let err = export_fast(FastExport {
            source: &clip,
            dest: &spelled_differently,
            start_ms: 0,
            end_ms: 1_000,
        })
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("ne peut pas être le clip source"),
            "the refusal must name the reason: {err:#}"
        );
        assert_eq!(
            std::fs::read(&clip).unwrap(),
            STUB,
            "the source clip must come out of a refused export byte for byte"
        );
        // The other spelling Windows lets through: same name, different case.
        assert!(
            names_the_same_file(&dir.join("CLIP.MP4"), &clip),
            "a case-different path names the same file on this filesystem"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the same guard: an ordinary export writes to a path
    /// that does not exist yet, where `canonicalize` fails on the destination.
    /// If that made the check say "same file", every export would be refused.
    #[test]
    fn a_destination_that_does_not_exist_yet_is_not_the_source() {
        let dir = scratch_dir("distinct");
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, STUB).unwrap();
        let dest = dir.join("export.mp4");

        assert!(!names_the_same_file(&dest, &clip), "the ordinary export must not be refused");
        assert!(names_the_same_file(&clip, &clip), "the identical spelling is still caught");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pre-creation window, end to end through the public function: the
    /// source does not exist, so `keyframes_ms` fails before any sink writer
    /// is created, and the file already sitting at `dest` must be left exactly
    /// as it was. (No Media Foundation involved -- `open_compressed` refuses a
    /// path that is not a file before it starts MF.)
    #[test]
    fn a_source_that_cannot_be_read_leaves_the_destination_untouched() {
        let dir = scratch_dir("no-source");
        let dest = dir.join("out.mp4");
        std::fs::write(&dest, STUB).unwrap();
        let missing = dir.join("nope.mp4");

        let err =
            export_fast(FastExport { source: &missing, dest: &dest, start_ms: 0, end_ms: 1_000 })
                .unwrap_err();

        assert!(format!("{err:#}").contains("nope.mp4"), "the error must name the source: {err:#}");
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            STUB,
            "a failure before `dest` was ever created must not delete what is there"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs against a real clip, so it is `#[ignore]`d: `cargo test` on a
    /// machine with no clips must not fail, and this needs Media Foundation,
    /// a real H.264 file, and a path only the developer knows.
    ///
    /// Run it with:
    ///   cargo test -p clippy-core -- --ignored keyframe_index_of_a_real_clip
    /// after setting CLIPPY_TEST_CLIP to the full path of an .mp4 in your library.
    #[test]
    #[ignore = "needs a real clip; set CLIPPY_TEST_CLIP"]
    fn keyframe_index_of_a_real_clip() {
        let Ok(path) = std::env::var("CLIPPY_TEST_CLIP") else {
            panic!("set CLIPPY_TEST_CLIP to an .mp4 path");
        };
        let keys = keyframes_ms(Path::new(&path)).expect("indexing a real clip");
        assert_eq!(keys.first(), Some(&0), "a clip always has a keyframe at 0");
        assert!(keys.len() > 1, "a GOP-pinned clip has one keyframe per second: {keys:?}");
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "ascending and unique: {keys:?}");
    }

    /// The other half of the by-hand gate. Exports the middle of a real clip
    /// and asserts the output is a real, shorter, independently indexable MP4
    /// -- and, when the source carries audio, that the audio came with it.
    ///
    /// That last assertion is the only guard `has_audio` has. Its real
    /// computation (`audio.packets > 0`, in `run_export`) needs Media
    /// Foundation and a real AAC track to exercise, so nothing in the ordinary
    /// suite touches it: reverting it to `audio_out.is_some()` -- the exact
    /// regression an earlier fix wave corrected, which reports sound on a clip
    /// that has none -- leaves `cargo test --workspace` entirely green. It also
    /// covers the one thing this feature could not verify up front, which is
    /// whether AAC survives passthrough at all on a given machine.
    ///
    ///   cargo test -p clippy-core -- --ignored fast_export_of_a_real_clip
    #[test]
    #[ignore = "needs a real clip; set CLIPPY_TEST_CLIP"]
    fn fast_export_of_a_real_clip() {
        let Ok(path) = std::env::var("CLIPPY_TEST_CLIP") else {
            panic!("set CLIPPY_TEST_CLIP to an .mp4 path");
        };
        let source = Path::new(&path);
        let dir = scratch_dir("real-clip");
        let dest = dir.join("clippy-export-test.mp4");

        let outcome =
            export_fast(FastExport { source, dest: &dest, start_ms: 1_000, end_ms: 3_000 })
                .expect("exporting a real clip");

        assert!(outcome.video_packets > 0, "an export with no video is a failure");
        assert!(dest.is_file(), "the output must exist");
        assert!(
            std::fs::metadata(&dest).unwrap().len() < std::fs::metadata(source).unwrap().len(),
            "a 2 s trim of a longer clip must be smaller than the source"
        );
        // The proof it is a valid MP4 and not just bytes: re-index it.
        let keys = keyframes_ms(&dest).expect("the export must itself be readable");
        assert_eq!(keys.first(), Some(&0), "the export must start on a keyframe");

        // Guarded on the source, because CLIPPY_TEST_CLIP may legitimately point
        // at a clip recorded with the microphone and system audio both off.
        // Asked through `prepare`, which is the same call the export itself
        // uses to decide whether to declare an audio stream, so the two can
        // never disagree about what "the source has audio" means.
        let (_reader, _video, audio_type) = prepare(source).expect("the source must at least open");
        if !audio_type.is_empty() {
            assert!(outcome.has_audio, "the source has audio tracks but the export reported none");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A range that snaps to a keyframe at or after every sample in it (here,
    /// `end_ms: 0` against a clip whose first frame is at `pts_100ns == 0`)
    /// writes zero video packets, and a sink handed no samples at all fails
    /// `Finalize` with MF_E_SINK_NO_SAMPLES_PROCESSED. So this exercises the
    /// real failure path end to end -- no synthetic media types, just a range
    /// guaranteed to be empty -- and checks both halves of the answer: the
    /// caller-facing message, and that nothing broken is left on disk for
    /// `library::scan` to adopt.
    ///
    /// `dest` is seeded first, with content nothing else would write. Its
    /// absence at the end can then only be explained by a delete: an assertion
    /// against a path the test itself had cleared could not tell "the export
    /// created a file and cleaned it up" from "the export never got that far".
    ///
    ///   cargo test -p clippy-core -- --ignored a_video_less_export_deletes_its_own_output_and_names_the_reason
    #[test]
    #[ignore = "needs a real clip; set CLIPPY_TEST_CLIP"]
    fn a_video_less_export_deletes_its_own_output_and_names_the_reason() {
        let Ok(path) = std::env::var("CLIPPY_TEST_CLIP") else {
            panic!("set CLIPPY_TEST_CLIP to an .mp4 path");
        };
        let source = Path::new(&path);
        let dir = scratch_dir("empty-range");
        let dest = dir.join("clippy-export-empty-test.mp4");
        std::fs::write(&dest, STUB).unwrap();

        let err = export_fast(FastExport { source, dest: &dest, start_ms: 0, end_ms: 0 })
            .expect_err("a zero-length range must not produce a clip");
        assert!(
            format!("{err:#}").contains("aucune vidéo"),
            "the caller must see why, not a raw HRESULT: {err:#}"
        );
        assert!(
            !dest.exists(),
            "the seeded file was truncated by MFCreateSinkWriterFromURL and then had to be \
             deleted; finding the path gone is the proof the delete ran"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Frequency of a stereo i16 PCM buffer, from its zero crossings. Good
    /// enough to tell the test tones (220, 440, 660 Hz) apart.
    fn pitch_hz(pcm: &[u8]) -> f64 {
        let left: Vec<i16> =
            pcm.chunks_exact(4).map(|f| i16::from_le_bytes([f[0], f[1]])).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        let seconds = left.len() as f64 / crate::capture::audio::SAMPLE_RATE as f64;
        crossings as f64 / 2.0 / seconds
    }

    /// A software-encoded fixture: no screen capture, user media or GPU needed.
    fn synthetic_video(path: &Path) {
        synthetic_video_at_fps(path, 10);
    }

    fn synthetic_video_at_fps(path: &Path, fps: u32) {
        use crate::capture::audio::SAMPLE_RATE;
        use crate::encode::mf::{RecorderSettings, add_aac_stream, video_type, write_pcm_sample};
        use windows::Win32::Media::MediaFoundation::{
            MFCreateMemoryBuffer, MFCreateSample, MFVideoFormat_H264, MFVideoFormat_NV12,
        };
        ensure_mf_started().expect("MF startup");
        let settings = RecorderSettings {
            src_width: 160,
            src_height: 96,
            width: 160,
            height: 96,
            fps,
            bitrate_bps: 300_000,
            max_bitrate_bps: 300_000,
            rate_control: crate::config::RateControl::Cbr,
            with_audio: true,
        };
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)
                .expect("software fixture writer");
            let video = writer
                .AddStream(&video_type(&settings, &MFVideoFormat_H264, true).unwrap())
                .expect("H264 output");
            writer
                .SetInputMediaType(
                    video,
                    &video_type(&settings, &MFVideoFormat_NV12, false).unwrap(),
                    None,
                )
                .expect("software NV12 input");
            let audio: Vec<_> = (0..3).map(|_| add_aac_stream(&writer).unwrap()).collect();
            writer.BeginWriting().expect("fixture begin");
            for frame in 0..fps * 3 {
                let mut pixels = vec![128u8; 160 * 96 * 3 / 2];
                pixels[..160 * 96].fill(32 + (frame % 30) as u8 * 5);
                let buffer = MFCreateMemoryBuffer(pixels.len() as u32).unwrap();
                let mut data = std::ptr::null_mut();
                buffer.Lock(&mut data, None, None).unwrap();
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len());
                buffer.Unlock().unwrap();
                buffer.SetCurrentLength(pixels.len() as u32).unwrap();
                let sample = MFCreateSample().unwrap();
                sample.AddBuffer(&buffer).unwrap();
                let pts = i64::from(frame) * 10_000_000 / i64::from(fps);
                let next = i64::from(frame + 1) * 10_000_000 / i64::from(fps);
                sample.SetSampleTime(pts).unwrap();
                sample.SetSampleDuration(next - pts).unwrap();
                writer.WriteSample(video, &sample).expect("fixture video frame");
                for (ordinal, stream) in audio.iter().enumerate() {
                    let mut pcm = Vec::with_capacity(SAMPLE_RATE / fps as usize * 4);
                    for n in 0..SAMPLE_RATE / fps as usize {
                        let t = (frame as usize * (SAMPLE_RATE / fps as usize) + n) as f64
                            / SAMPLE_RATE as f64;
                        let game = (t * 440.0 * std::f64::consts::TAU).sin() * 3000.0;
                        let chat = (t * 660.0 * std::f64::consts::TAU).sin() * 3000.0;
                        let value = match ordinal {
                            0 => game + chat,
                            1 => game,
                            _ => chat,
                        } as i16;
                        pcm.extend_from_slice(&value.to_le_bytes());
                        pcm.extend_from_slice(&value.to_le_bytes());
                    }
                    write_pcm_sample(&writer, *stream, pts, &pcm).expect("fixture audio frame");
                }
            }
            writer.Finalize().expect("fixture finalize");
        }
    }

    fn tone_level(pcm: &[u8], hz: f64) -> f64 {
        let frames: Vec<_> = pcm.chunks_exact(4).collect();
        let middle = &frames[frames.len() / 4..frames.len() * 3 / 4];
        let mut real = 0.0;
        let mut imaginary = 0.0;
        for (n, frame) in middle.iter().enumerate() {
            let sample = f64::from(i16::from_le_bytes([frame[0], frame[1]]));
            let angle =
                n as f64 * hz * std::f64::consts::TAU / crate::capture::audio::SAMPLE_RATE as f64;
            real += sample * angle.cos();
            imaginary += sample * angle.sin();
        }
        2.0 * real.hypot(imaginary) / middle.len() as f64
    }

    fn decoded_video_frames(path: &Path) -> Vec<(i64, i64, u8)> {
        use windows::Win32::Media::MediaFoundation::{
            MF_MT_SUBTYPE, MFCreateMediaType, MFMediaType_Video, MFVideoFormat_NV12,
        };
        let reader = open_compressed(path).unwrap();
        unsafe {
            let media_type = MFCreateMediaType().unwrap();
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
            reader.SetCurrentMediaType(video_stream(), None, &media_type).unwrap();
        }
        let mut frames = Vec::new();
        loop {
            let mut sample = None;
            let mut flags = 0;
            unsafe {
                reader.ReadSample(
                    video_stream(),
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
            }
            .unwrap();
            if let Some(sample) = sample {
                unsafe {
                    let buffer = sample.ConvertToContiguousBuffer().unwrap();
                    let mut data = std::ptr::null_mut();
                    let mut len = 0;
                    buffer.Lock(&mut data, None, Some(&mut len)).unwrap();
                    assert!(len > 0);
                    let luma = *data;
                    buffer.Unlock().unwrap();
                    frames.push((
                        sample.GetSampleTime().unwrap(),
                        sample.GetSampleDuration().unwrap(),
                        luma,
                    ));
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        frames
    }

    #[test]
    fn precise_native_export_writes_selected_first_and_last_images_and_audio_policy() {
        let dir = scratch_dir("precise-native");
        let source = dir.join("source.mp4");
        synthetic_video(&source);
        let original = std::fs::read(&source).unwrap();
        let original_frames = decoded_video_frames(&source);
        let index = frame_index(&source).unwrap();
        assert_eq!(index.timestamps_100ns.len(), 30);
        for (name, output, tracks, count) in [
            ("master", RemixOutput::MixAndStems, None, 3),
            ("share", RemixOutput::MixOnly, None, 1),
            ("silent", RemixOutput::MixOnly, Some(&[][..]), 0),
        ] {
            let dest = dir.join(format!("{name}.mp4"));
            let outcome = export_precise(PreciseExport {
                source: &source,
                dest: &dest,
                start_ms: 550,
                end_ms: 1550,
                tracks,
                output,
                size_limit_bytes: None,
                vertical_center: None,
            })
            .unwrap();
            assert_eq!(outcome.video_packets, 10);
            assert_eq!(outcome.duration_ms, 1000);
            assert_eq!(audio_track_count(&dest).unwrap(), count);
            let frames = decoded_video_frames(&dest);
            assert_eq!(frames.len(), 10);
            assert_eq!(frames[0].0, 0);
            assert_eq!(frames.last().unwrap().0 + frames.last().unwrap().1, 10_000_000);
            assert!(
                (i16::from(frames[0].2) - i16::from(original_frames[5].2)).abs() <= 2,
                "first decoded image must be source frame 5: {:?}",
                frames[0]
            );
            assert!(
                (i16::from(frames[9].2) - i16::from(original_frames[14].2)).abs() <= 2,
                "last decoded image must be source frame 14: {:?}",
                frames[9]
            );
            if count > 0 {
                let (pcm, offset) = decode_track(&dest, 0, 0, i64::MAX).unwrap();
                assert!(offset < 209, "audio should start within one PCM frame");
                let frames = pcm.len() / 4;
                // AAC writes whole 1024-frame blocks; decode the full output
                // instead of masking this encoder padding with a trim.
                assert!(
                    (48_000..=48_000 + 2048).contains(&frames),
                    "unexpected AAC duration: {frames} PCM frames"
                );
            }
        }
        let dest = dir.join("cancelled.mp4");
        let control = ExportControl::new();
        control.cancel_at.store(55, Ordering::Relaxed);
        let error = export_precise_with_control(
            PreciseExport {
                source: &source,
                dest: &dest,
                start_ms: 550,
                end_ms: 1550,
                tracks: None,
                output: RemixOutput::MixOnly,
                size_limit_bytes: None,
                vertical_center: None,
            },
            &control,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<ExportCancelled>().is_some());
        assert!(!dest.exists());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_pcm_trim_keeps_only_samples_inside_both_partial_aac_packets() {
        let dir = scratch_dir("precise-pcm");
        let source = dir.join("source.mp4");
        synthetic_video(&source);
        for ordinal in 0..3 {
            let (full, _) = decode_track(&source, ordinal, 0, i64::MAX).unwrap();
            // 0.5 s is not an AAC 1024-frame packet boundary at 48 kHz.
            let (trimmed, offset) = decode_track(&source, ordinal, 5_000_000, 15_000_000).unwrap();
            assert!(offset < 209, "first PCM frame must align within one sample");
            assert_eq!(trimmed.len(), 48_000 * 4);
            assert_eq!(
                trimmed,
                full[24_000 * 4..72_000 * 4],
                "partial AAC packets must be cropped before mixing"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn precise_native_sixty_fps_rounding_keeps_the_requested_frame() {
        let dir = scratch_dir("precise-sixty");
        let source = dir.join("sixty.mp4");
        let dest = dir.join("two-frames.mp4");
        synthetic_video_at_fps(&source, 60);
        let source_frames = decoded_video_frames(&source);
        let outcome = export_precise(PreciseExport {
            source: &source,
            dest: &dest,
            start_ms: 17,
            end_ms: 50,
            tracks: Some(&[]),
            output: RemixOutput::MixOnly,
            size_limit_bytes: None,
            vertical_center: None,
        })
        .unwrap();
        assert_eq!(outcome.video_packets, 2);
        let frames = decoded_video_frames(&dest);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, 0);
        assert!((i16::from(frames[0].2) - i16::from(source_frames[1].2)).abs() <= 2);
        assert!((i16::from(frames[1].2) - i16::from(source_frames[2].2)).abs() <= 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Opt-in: `CLIPPY_REAL_CLIP=<mp4> CLIPPY_REAL_OUT=<mp4>` fits 30 s of a
    /// real recording into 20 MB. Not run by default: needs a real clip.
    #[test]
    #[ignore]
    fn real_clip_fits_the_discord_free_cap() {
        let (Ok(source), Ok(dest)) =
            (std::env::var("CLIPPY_REAL_CLIP"), std::env::var("CLIPPY_REAL_OUT"))
        else {
            return;
        };
        let started = std::time::Instant::now();
        let outcome = export_precise(PreciseExport {
            source: Path::new(&source),
            dest: Path::new(&dest),
            start_ms: 0,
            end_ms: if std::env::var("CLIPPY_REAL_NOLIMIT").is_ok() { 3_000 } else { 30_000 },
            tracks: None,
            output: RemixOutput::MixOnly,
            size_limit_bytes: std::env::var("CLIPPY_REAL_NOLIMIT").is_err().then_some(20_000_000),
            vertical_center: std::env::var("CLIPPY_REAL_VERTICAL").ok().and_then(|v| v.parse().ok()),
        })
        .unwrap();
        let bytes = std::fs::metadata(&dest).unwrap().len();
        println!("{outcome:?} {bytes} bytes in {:?}", started.elapsed());
        assert!(bytes <= 20_000_000);
    }

    #[test]
    fn precise_native_size_cap_lands_under_the_limit() {
        let dir = scratch_dir("precise-size-cap");
        let source = dir.join("source.mp4");
        synthetic_video_at_fps(&source, 30);
        let original = std::fs::read(&source).unwrap();
        // 3 s at 160x96 into 120 kB: ~288 kb/s for a silent share.
        let dest = dir.join("capped.mp4");
        let outcome = export_precise(PreciseExport {
            source: &source,
            dest: &dest,
            start_ms: 0,
            end_ms: 3000,
            tracks: Some(&[]),
            output: RemixOutput::MixOnly,
            size_limit_bytes: Some(120_000),
            vertical_center: None,
        })
        .unwrap();
        assert_eq!(outcome.video_packets, 90);
        assert!(std::fs::metadata(&dest).unwrap().len() <= 120_000);
        assert_eq!(decoded_video_frames(&dest).len(), 90);
        // The resize path the plan picks for large sources: the reader's video
        // processor scales, and every frame still lands at its own time.
        let index = frame_index(&source).unwrap();
        let resized = dir.join("resized.mp4");
        let request = PreciseExport {
            source: &source,
            dest: &resized,
            start_ms: 0,
            end_ms: 1000,
            tracks: Some(&[]),
            output: RemixOutput::MixOnly,
            size_limit_bytes: None,
            vertical_center: None,
        };
        let outcome = precise::write_precise(
            &request,
            &ExportControl::new(),
            &index,
            (0, 10_000_000),
            &[],
            200_000,
            Some(precise::Framing { crop: (0, 0, 160, 96), size: (80, 48) }),
        )
        .unwrap();
        assert_eq!(outcome.frame_size, Some((80, 48)));
        assert_eq!(outcome.video_packets, 30);
        let native = unsafe {
            open_compressed(&resized).unwrap().GetNativeMediaType(video_stream(), 0).unwrap()
        };
        let size = unsafe {
            native.GetUINT64(&windows::Win32::Media::MediaFoundation::MF_MT_FRAME_SIZE).unwrap()
        };
        assert_eq!(((size >> 32) as u32, size as u32), (80, 48));
        // Portrait: a 9:16 column of the 160x96 source, every frame kept.
        let vertical = dir.join("vertical.mp4");
        let outcome = export_precise(PreciseExport {
            source: &source,
            dest: &vertical,
            start_ms: 0,
            end_ms: 1000,
            tracks: None,
            output: RemixOutput::MixOnly,
            size_limit_bytes: None,
            vertical_center: Some(500),
        })
        .unwrap();
        assert_eq!(outcome.frame_size, Some((54, 96)));
        assert_eq!(decoded_video_frames(&vertical).len(), 30);
        // A cap that cannot hold even the floor bitrate is refused up front
        // and leaves nothing behind.
        let refused = dir.join("refused.mp4");
        let error = export_precise(PreciseExport {
            source: &source,
            dest: &refused,
            start_ms: 0,
            end_ms: 3000,
            tracks: None,
            output: RemixOutput::MixOnly,
            size_limit_bytes: Some(20_000),
            vertical_center: None,
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("trop longue"), "{error:#}");
        assert!(!refused.exists());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn synthetic_video_exports_master_and_single_mix_share_end_to_end() {
        let dir = scratch_dir("synthetic-video");
        let source = dir.join("source.mp4");
        synthetic_video(&source);
        let source_bytes = std::fs::read(&source).unwrap();
        let master = dir.join("master.mp4");
        let fast =
            export_fast(FastExport { source: &source, dest: &master, start_ms: 0, end_ms: 3000 })
                .expect("full MP4 master export");
        assert_eq!(audio_track_count(&master).unwrap(), 3);
        assert!(fast.video_packets >= 29 && (2900..=3100).contains(&fast.duration_ms));
        for (ordinal, hz) in [(1, 440.0), (2, 660.0)] {
            let (pcm, _) = decode_track(&master, ordinal, 0, i64::MAX).unwrap();
            assert!((pitch_hz(&pcm[pcm.len() / 4..pcm.len() * 3 / 4]) - hz).abs() < 5.0);
        }
        let share = dir.join("share.mp4");
        let tracks = [
            TrackChoice { ordinal: 1, gain_percent: 100 },
            TrackChoice { ordinal: 2, gain_percent: 50 },
        ];
        let mixed = export_remix(RemixExport {
            source: &source,
            dest: &share,
            start_ms: 0,
            end_ms: 3000,
            tracks: &tracks,
            output: RemixOutput::MixOnly,
        })
        .expect("full MP4 mixed share export");
        assert_eq!(audio_track_count(&share).unwrap(), 1);
        assert!(mixed.video_packets >= 29 && (2900..=3100).contains(&mixed.duration_ms));
        let (pcm, _) = decode_track(&share, 0, 0, i64::MAX).unwrap();
        let game = tone_level(&pcm, 440.0);
        let chat = tone_level(&pcm, 660.0);
        assert!((2500.0..3500.0).contains(&game), "game level: {game}");
        assert!((0.20..0.30).contains(&(chat / game)), "chat/game ratio: {}", chat / game);
        assert_eq!(audio_track_count(&source).unwrap(), 3, "source remains editable");
        assert_eq!(
            std::fs::read(&source).unwrap(),
            source_bytes,
            "exports must leave the source byte-for-byte unchanged"
        );
        // Manual browser playback assets are persisted only into a directory
        // explicitly selected by the caller. Ordinary test runs own only their
        // scratch directory and never overwrite a shared temp filename.
        if let Some(preview_dir) = std::env::var_os("CLIPPY_TEST_PREVIEW_DIR") {
            let preview_dir = std::path::PathBuf::from(preview_dir);
            std::fs::create_dir_all(&preview_dir).unwrap();
            std::fs::copy(&master, preview_dir.join("editor-preview.mp4")).unwrap();
            for ordinal in 1..=2 {
                extract_audio_track(
                    &master,
                    ordinal,
                    &preview_dir.join(format!("editor-preview.t{ordinal}.m4a")),
                )
                .expect("persist preview audio stem");
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The bug this guards: the editor's track `n` must be the stream written
    /// as track `n`. Reading the streams back in another order put the game's
    /// sound in the chat's track and the other way round.
    #[test]
    fn each_track_comes_back_as_the_stream_written_as_it() {
        use crate::capture::audio::{ENCODER_BLOCK_ALIGN, SAMPLE_RATE};
        use crate::encode::mf::{add_aac_stream, ensure_mf_started, write_pcm_sample};
        use windows::Win32::Media::MediaFoundation::MFCreateSinkWriterFromURL;
        ensure_mf_started().expect("MF");
        let path = std::env::temp_dir().join(format!("clippy-order-{}.m4a", std::process::id()));
        let pitches = [220.0f32, 440.0, 660.0];
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)
                .expect("writer");
            let streams: Vec<u32> =
                pitches.iter().map(|_| add_aac_stream(&writer).expect("aac")).collect();
            writer.BeginWriting().expect("begin");
            for (&stream, &hz) in streams.iter().zip(&pitches) {
                let frames = SAMPLE_RATE * 2;
                let mut pcm = Vec::with_capacity(frames * ENCODER_BLOCK_ALIGN);
                for n in 0..frames {
                    let v = ((n as f32 * hz * std::f32::consts::TAU / SAMPLE_RATE as f32).sin()
                        * 6000.0) as i16;
                    pcm.extend_from_slice(&v.to_le_bytes());
                    pcm.extend_from_slice(&v.to_le_bytes());
                }
                for (k, chunk) in pcm.chunks(SAMPLE_RATE * ENCODER_BLOCK_ALIGN).enumerate() {
                    write_pcm_sample(&writer, stream, k as i64 * 10_000_000, chunk).expect("write");
                }
            }
            writer.Finalize().expect("finalize");
        }
        let mut found = Vec::new();
        for ordinal in 0..pitches.len() {
            let (pcm, _) = decode_track(&path, ordinal, 0, i64::MAX).expect("decode");
            // Skip the encoder's priming at each end.
            let middle = &pcm[pcm.len() / 4..pcm.len() * 3 / 4];
            found.push(pitch_hz(middle).round() as i64);
        }
        assert_eq!(found, [220, 440, 660], "ordinal n must decode as the n-th stream written");

        // Exercise the same stream declaration/copy used by the default fast
        // trim. Its output must retain every independently editable track.
        let dest = path.with_file_name(format!("clippy-order-copy-{}.m4a", std::process::id()));
        let reader = open_reader(&path).expect("reader");
        let audio_types: Vec<_> = audio_stream_indices(&reader)
            .expect("enumerate")
            .into_iter()
            .map(|stream| {
                unsafe { reader.SetStreamSelection(stream, true) }.expect("select");
                (stream, unsafe { reader.GetNativeMediaType(stream, 0) }.expect("native"))
            })
            .collect();
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dest.as_os_str()), None, None)
                .expect("copy writer");
            let outputs = add_audio_passthrough(&writer, &audio_types).expect("declare all tracks");
            writer.BeginWriting().expect("begin copy");
            assert!(
                copy_audio_streams(&reader, &writer, &audio_types, &outputs, 0, 10_000_000, &path)
                    .expect("copy all tracks")
            );
            writer.Finalize().expect("finalize copy");
        }
        assert_eq!(audio_track_count(&dest).expect("count copied tracks"), pitches.len());
        for (ordinal, hz) in pitches.iter().enumerate() {
            let (pcm, _) = decode_track(&dest, ordinal, 0, i64::MAX).expect("decode copy");
            let middle = &pcm[pcm.len() / 4..pcm.len() * 3 / 4];
            assert!(
                (pitch_hz(middle) - f64::from(*hz)).abs() < 5.0,
                "fast export changed the identity of track {ordinal}"
            );
        }
        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_file(&path);
    }
}
