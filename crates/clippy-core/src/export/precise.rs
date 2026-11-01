//! Frame-boundary export. Video is decoded to NV12 and encoded to H.264;
//! audio is trimmed in PCM before encoding, so neither depends on keyframes.
use super::*;
use crate::encode::mf::{add_aac_stream, write_pcm_sample};
use windows::Win32::Media::MediaFoundation::{
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MFCreateMediaType, MFMediaType_Video,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameIndex {
    pub timestamps_100ns: Vec<i64>,
    /// Exclusive end of the final video frame (not the longer audio duration).
    pub end_100ns: i64,
}

/// How far a decoded timestamp may sit from its index entry (1 ms).
const PTS_TOLERANCE_100NS: i64 = 10_000;

impl FrameIndex {
    /// Position of the stored frame time nearest `pts`, if one is within
    /// [`PTS_TOLERANCE_100NS`].
    fn nearest_index(&self, pts: i64) -> Option<usize> {
        let frames = &self.timestamps_100ns;
        let at = frames.partition_point(|&t| t < pts);
        [at.checked_sub(1), Some(at)]
            .into_iter()
            .flatten()
            .filter(|&i| i < frames.len())
            .min_by_key(|&i| (frames[i] - pts).abs())
            .filter(|&i| (frames[i] - pts).abs() <= PTS_TOLERANCE_100NS)
    }

    fn nearest(&self, requested: i64, include_end: bool) -> i64 {
        let frames = &self.timestamps_100ns;
        let index = frames.partition_point(|&pts| pts < requested);
        let before = index.checked_sub(1).map(|i| frames[i]);
        let after = frames.get(index).copied().or(include_end.then_some(self.end_100ns));
        match (before, after) {
            (Some(a), Some(b)) if requested.saturating_sub(a) <= b.saturating_sub(requested) => a,
            (_, Some(b)) => b,
            (Some(a), None) => a,
            _ => frames[0],
        }
    }

    fn range(&self, start_ms: u64, end_ms: u64) -> Result<(i64, i64)> {
        let ticks = |ms: u64| -> Result<i64> {
            i64::try_from(ms)
                .ok()
                .and_then(|ms| ms.checked_mul(10_000))
                .context("la position de coupe dépasse la durée prise en charge")
        };
        if end_ms <= start_ms {
            anyhow::bail!("la fin doit être après le début");
        }
        let start = self.nearest(ticks(start_ms)?, false);
        let end = self.nearest(ticks(end_ms)?, true);
        if end <= start {
            anyhow::bail!("cette plage ne contient aucune image complète");
        }
        Ok((start, end))
    }
}

pub fn frame_index(source: &Path) -> Result<FrameIndex> {
    frame_index_controlled(source, &ExportControl::new())
}

fn frame_index_controlled(source: &Path, control: &ExportControl) -> Result<FrameIndex> {
    control.check()?;
    let reader = open_compressed(source)?;
    let mut timestamps = Vec::new();
    let mut end = 0;
    let mut empty = 0;
    loop {
        control.check()?;
        let mut flags = 0;
        let mut sample = None;
        unsafe {
            reader.ReadSample(video_stream(), 0, None, Some(&mut flags), None, Some(&mut sample))
        }
        .context("lecture de l’index vidéo")?;
        if let Some(sample) = sample {
            empty = 0;
            let pts = unsafe { sample.GetSampleTime() }.context("image sans timestamp")?;
            let duration = unsafe { sample.GetSampleDuration() }.context("image sans durée")?;
            if pts < 0 || duration <= 0 {
                anyhow::bail!("timing vidéo invalide");
            }
            end = end.max(pts.checked_add(duration).context("timing vidéo trop grand")?);
            timestamps.push(pts);
        } else if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 == 0 {
            empty = count_empty_read(empty, source, "video")?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
    }
    timestamps.sort_unstable();
    timestamps.dedup();
    if timestamps.is_empty() {
        anyhow::bail!("ce clip ne contient aucune image vidéo");
    }
    control.report(ExportStage::Indexing, 10)?;
    Ok(FrameIndex { timestamps_100ns: timestamps, end_100ns: end })
}

pub struct PreciseExport<'a> {
    pub source: &'a Path,
    pub dest: &'a Path,
    pub start_ms: u64,
    pub end_ms: u64,
    /// None retains the source mix/stems without mixing them together again.
    pub tracks: Option<&'a [TrackChoice]>,
    pub output: RemixOutput,
    /// Largest file allowed, in bytes. The video bitrate, and the picture
    /// size when the bitrate alone would look poor, follow [`share_plan`].
    pub size_limit_bytes: Option<u64>,
    /// Some(c) crops a 9:16 portrait picture centred `c`/1000 of the way
    /// across the source (500 = centre); None keeps the source framing.
    pub vertical_center: Option<u16>,
}

/// Source rectangle kept and size it is written at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Framing {
    /// x, y, width, height inside the visible picture, all even.
    pub crop: (u32, u32, u32, u32),
    pub size: (u32, u32),
}

/// The 9:16 rectangle of a `width`×`height` picture centred `center`/1000 of
/// the way across, clamped inside the picture. Even edges keep NV12's chroma
/// pairs whole. A source already narrower than 9:16 is kept whole.
pub fn vertical_crop(width: u32, height: u32, center: u16) -> (u32, u32, u32, u32) {
    let crop_w = ((u64::from(height) * 9 / 16) as u32).min(width) & !1;
    let centre = u64::from(width) * u64::from(center.min(1000)) / 1000;
    let x = (centre.saturating_sub(u64::from(crop_w) / 2) as u32).min(width - crop_w) & !1;
    (x, 0, crop_w, height & !1)
}

/// Bitrate of every AAC stream the exporter writes (`add_aac_stream`'s
/// 24 000 bytes per second).
const AAC_BPS: u64 = 192_000;
/// Share of the size cap the first attempt aims for. The rest covers the MP4
/// index, AAC priming and the encoder's overshoot on hard frames.
const SIZE_HEADROOM_PERCENT: u64 = 90;
/// Below this the video is a smear at any picture size worth sharing.
const MIN_VIDEO_BPS: u64 = 150_000;
/// Bits per pixel per frame under which H.264 game footage visibly blocks,
/// so the picture is made smaller instead (0.04 ≈ 5 Mb/s at 1080p60).
const MIN_BITS_PER_PIXEL_MILLI: u64 = 40;
/// Short sides tried, largest first, when the source is too big for the budget.
const SHORT_SIDES: [u32; 5] = [1080, 720, 540, 480, 360];
/// Encodes after the first one when the file still came out over the cap.
const SIZE_RETRIES: usize = 2;

/// The encode settings that fit a clip into a size cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharePlan {
    pub video_bps: u32,
    pub width: u32,
    pub height: u32,
}

/// Chooses a video bitrate and picture size so `duration_100ns` of video plus
/// `audio_streams` AAC streams fits in `limit_bytes`.
///
/// Keeps the source size when it gets at least [`MIN_BITS_PER_PIXEL_MILLI`],
/// otherwise steps down through [`SHORT_SIDES`] (never upscaling), keeping
/// the aspect ratio with even dimensions. `source_bps` caps the bitrate: a
/// generous cap must not inflate a clip past its own quality.
pub fn share_plan(
    width: u32,
    height: u32,
    fps_milli: u64,
    duration_100ns: i64,
    audio_streams: usize,
    limit_bytes: u64,
    source_bps: Option<u64>,
) -> Result<SharePlan> {
    if width < 2 || height < 2 || duration_100ns <= 0 {
        anyhow::bail!("dimensions ou durée vidéo invalides");
    }
    let audio = AAC_BPS * audio_streams as u64;
    let budget_bits = u128::from(limit_bytes) * 8 * u128::from(SIZE_HEADROOM_PERCENT) / 100;
    let total_bps = (budget_bits * 10_000_000 / duration_100ns as u128) as u64;
    let video = total_bps.saturating_sub(audio);
    if video < MIN_VIDEO_BPS {
        let longest_s = budget_bits / u128::from((MIN_VIDEO_BPS + audio) * 10).max(1) / 10;
        anyhow::bail!(
            "cette sélection est trop longue pour tenir dans {} Mo : raccourcissez-la à environ {longest_s} s",
            limit_bytes / 1_000_000
        );
    }
    let video = video.min(source_bps.filter(|&b| b > 0).unwrap_or(u64::MAX)).min(80_000_000);
    let fps_milli = fps_milli.max(1);
    // bits per pixel per frame, in thousandths
    let bpp_milli = |w: u32, h: u32| {
        u128::from(video) * 1_000_000 / (u128::from(w) * u128::from(h) * u128::from(fps_milli))
    };
    let (short, long) = (width.min(height), width.max(height));
    let mut chosen = (width, height);
    if bpp_milli(width, height) < u128::from(MIN_BITS_PER_PIXEL_MILLI) {
        for side in SHORT_SIDES.into_iter().filter(|&s| s < short) {
            let other = ((u64::from(long) * u64::from(side) + u64::from(short) / 2)
                / u64::from(short)) as u32
                & !1;
            chosen = if width >= height { (other, side) } else { (side, other) };
            if bpp_milli(chosen.0, chosen.1) >= u128::from(MIN_BITS_PER_PIXEL_MILLI) {
                break;
            }
        }
    }
    let (width, height) = (chosen.0 & !1, chosen.1 & !1);
    if width < 16 || height < 16 {
        anyhow::bail!("format d’image trop étiré pour réduire la vidéo ({width}×{height})");
    }
    Ok(SharePlan { video_bps: video as u32, width, height })
}

pub fn export_precise(request: PreciseExport<'_>) -> Result<ExportOutcome> {
    export_precise_with_control(request, &ExportControl::new())
}

/// Bilinear resample of one plane, `channels` interleaved bytes per pixel
/// (1 for luma, 2 for NV12's UV pairs), into a tightly packed `dst`.
fn scale_plane(
    src: &[u8],
    pitch: usize,
    (sw, sh): (usize, usize),
    dst: &mut [u8],
    (dw, dh): (usize, usize),
    channels: usize,
) {
    // 16.16 fixed point, sampling at pixel centres.
    let step_x = ((sw << 16) / dw.max(1)) as i64;
    let step_y = ((sh << 16) / dh.max(1)) as i64;
    let max_x = ((sw - 1) << 16) as i64;
    let max_y = ((sh - 1) << 16) as i64;
    for y in 0..dh {
        let fy = (y as i64 * step_y + step_y / 2 - 0x8000).clamp(0, max_y);
        let (y0, wy) = ((fy >> 16) as usize, (fy & 0xFFFF) as u64);
        let y1 = (y0 + 1).min(sh - 1);
        let (r0, r1) = (&src[y0 * pitch..], &src[y1 * pitch..]);
        let out = &mut dst[y * dw * channels..(y + 1) * dw * channels];
        for x in 0..dw {
            let fx = (x as i64 * step_x + step_x / 2 - 0x8000).clamp(0, max_x);
            let (x0, wx) = ((fx >> 16) as usize, (fx & 0xFFFF) as u64);
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..channels {
                let at = |row: &[u8], x: usize| u64::from(row[x * channels + c]);
                let top = at(r0, x0) * (0x10000 - wx) + at(r0, x1) * wx;
                let bottom = at(r1, x0) * (0x10000 - wx) + at(r1, x1) * wx;
                let value = (top * (0x10000 - wy) + bottom * wy + (1 << 31)) >> 32;
                out[x * channels + c] = value as u8;
            }
        }
    }
}

/// Resizes the `crop` rectangle (x, y, width, height; x and y even) of one
/// NV12 picture to `(dw, dh)`. `src` holds a luma plane of `pitch`-byte rows
/// and, `rows` rows in (the decoder may pad the height), the UV plane.
fn scale_nv12(
    src: &[u8],
    pitch: usize,
    rows: usize,
    crop: (u32, u32, u32, u32),
    (dw, dh): (u32, u32),
) -> Result<Vec<u8>> {
    let (x, y, sw, sh) = (crop.0 as usize, crop.1 as usize, crop.2 as usize, crop.3 as usize);
    let (dw, dh) = (dw as usize, dh as usize);
    if sw < 2
        || sh < 2
        || dw < 2
        || dh < 2
        || x % 2 != 0
        || y % 2 != 0
        || pitch < x + sw
        || rows < y + sh
        || src.len() < pitch * rows + pitch * ((y + sh) / 2)
    {
        anyhow::bail!("image NV12 inattendue ({} octets pour {sw}×{sh})", src.len());
    }
    let mut dst = vec![0u8; dw * dh * 3 / 2];
    let (luma, chroma) = dst.split_at_mut(dw * dh);
    scale_plane(&src[y * pitch + x..], pitch, (sw, sh), luma, (dw, dh), 1);
    // One UV pair per 2×2 luma block: x luma columns are x bytes of pairs.
    let uv = &src[pitch * rows + (y / 2) * pitch + x..];
    scale_plane(uv, pitch, (sw / 2, sh / 2), chroma, (dw / 2, dh / 2), 2);
    Ok(dst)
}

/// Where a decoded NV12 picture sits in its buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Nv12Layout {
    /// The picture to keep (the display aperture when the decoder pads).
    visible: (u32, u32),
    /// Rows of luma before the UV plane starts (the allocated height).
    rows: u32,
    /// Bytes per row for a contiguous buffer; a 2D buffer reports its own.
    stride: u32,
}

fn nv12_layout(decoded: &IMFMediaType) -> Result<Nv12Layout> {
    use windows::Win32::Media::MediaFoundation::{
        MF_MT_DEFAULT_STRIDE, MF_MT_MINIMUM_DISPLAY_APERTURE,
    };
    let (w, h) = unpack(unsafe { decoded.GetUINT64(&MF_MT_FRAME_SIZE) }?);
    // MFVideoArea: two MFOffset {fract: u16, value: i16}, then SIZE {cx, cy}.
    let mut area = [0u8; 16];
    let visible = match unsafe { decoded.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None) }
    {
        Ok(()) => {
            let word = |at: usize| u32::from_le_bytes(area[at..at + 4].try_into().unwrap());
            let (offset_x, offset_y) = (&area[0..4], &area[4..8]);
            if offset_x.iter().chain(offset_y).any(|&b| b != 0) {
                anyhow::bail!(
                    "image décodée décalée dans son tampon : redimensionnement impossible"
                );
            }
            (word(8).min(w), word(12).min(h))
        }
        Err(_) => (w, h),
    };
    let stride = unsafe { decoded.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .ok()
        .map(|s| s as i32)
        .filter(|&s| s >= w as i32)
        .map_or(w, |s| s as u32);
    Ok(Nv12Layout { visible, rows: h, stride })
}

/// A new sample holding `framing.crop` of `sample`'s picture at `framing.size`.
fn resized_sample(sample: &IMFSample, layout: Nv12Layout, framing: Framing) -> Result<IMFSample> {
    let (x, y, w, h) = framing.crop;
    if x + w > layout.visible.0 || y + h > layout.visible.1 {
        anyhow::bail!("recadrage hors de l’image décodée");
    }
    let size = framing.size;
    use windows::Win32::Media::MediaFoundation::{
        IMF2DBuffer2, MF2DBuffer_LockFlags_Read, MFCreateMemoryBuffer, MFCreateSample,
    };
    use windows::core::Interface as _;
    unsafe {
        // A 2D buffer knows its real pitch (decoders align rows, e.g. 1366
        // pixels in 1408-byte rows); only a plain buffer falls back to the
        // media type's stride.
        let single =
            (sample.GetBufferCount()? == 1).then(|| sample.GetBufferByIndex(0)).transpose()?;
        let two_d = single.as_ref().and_then(|b| b.cast::<IMF2DBuffer2>().ok());
        let scaled = if let Some(buffer) = two_d {
            let (mut scan0, mut pitch, mut start, mut len) =
                (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
            buffer.Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut len,
            )?;
            let result = if pitch <= 0 {
                Err(anyhow::anyhow!("image décodée à l’envers : redimensionnement impossible"))
            } else {
                let skip = (scan0 as usize).saturating_sub(start as usize);
                let bytes = std::slice::from_raw_parts(scan0, (len as usize).saturating_sub(skip));
                // The UV plane follows the *allocated* luma rows (1088 for a
                // 1080p H.264 picture), which only the buffer length tells.
                let pitch = pitch as usize;
                let rows = (bytes.len() * 2 / 3 / pitch).max(layout.rows as usize);
                scale_nv12(bytes, pitch, rows, framing.crop, size)
            };
            let _ = buffer.Unlock2D();
            result
        } else {
            let buffer =
                sample.ConvertToContiguousBuffer().context("lecture de l’image décodée")?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let bytes = std::slice::from_raw_parts(data, len as usize);
            let result = scale_nv12(
                bytes,
                layout.stride as usize,
                layout.rows as usize,
                framing.crop,
                size,
            );
            let _ = buffer.Unlock();
            result
        }?;
        let out = MFCreateMemoryBuffer(scaled.len() as u32)?;
        let mut target = std::ptr::null_mut();
        out.Lock(&mut target, None, None)?;
        std::ptr::copy_nonoverlapping(scaled.as_ptr(), target, scaled.len());
        out.Unlock()?;
        out.SetCurrentLength(scaled.len() as u32)?;
        let resized = MFCreateSample()?;
        resized.AddBuffer(&out)?;
        Ok(resized)
    }
}

fn unpack(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

pub fn export_precise_with_control(
    request: PreciseExport<'_>,
    control: &ExportControl,
) -> Result<ExportOutcome> {
    control.check()?;
    if names_the_same_file(request.dest, request.source) {
        anyhow::bail!("la destination de l’export ne peut pas être le clip source");
    }
    let index = frame_index_controlled(request.source, control)?;
    let (start, end) = index.range(request.start_ms, request.end_ms)?;
    let audio_count = audio_track_count(request.source)?;
    let selected: Vec<TrackChoice> = match request.tracks {
        Some(tracks) => tracks.to_vec(),
        None => (0..if request.output == RemixOutput::MixOnly {
            audio_count.min(1)
        } else {
            audio_count
        })
            .map(|ordinal| TrackChoice { ordinal, gain_percent: 100 })
            .collect(),
    };
    if selected.iter().any(|t| t.ordinal >= audio_count || t.gain_percent > 200) {
        anyhow::bail!("sélection audio invalide");
    }

    let native = unsafe { open_compressed(request.source)?.GetNativeMediaType(video_stream(), 0) }
        .context("type vidéo source")?;
    let (src_w, src_h) = unpack(unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) }?);
    let crop = match request.vertical_center {
        Some(center) => vertical_crop(src_w, src_h, center),
        None => (0, 0, src_w, src_h),
    };
    let source_bps = unsafe { native.GetUINT32(&MF_MT_AVG_BITRATE) }.ok().map(u64::from);
    let plan_for = |audio_streams: usize| -> Result<Option<SharePlan>> {
        let Some(limit) = request.size_limit_bytes else { return Ok(None) };
        let (num, den) = unpack(unsafe { native.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or(0));
        let fps_milli = if den == 0 { 60_000 } else { u64::from(num) * 1000 / u64::from(den) };
        share_plan(crop.2, crop.3, fps_milli, end - start, audio_streams, limit, source_bps)
            .map(Some)
    };
    // Refuse an impossible cap before the audio is decoded, with the fewest
    // streams the export could write; the real count is applied below.
    plan_for(0)?;

    control.report(ExportStage::Decoding, 10)?;
    let decoded = selected
        .iter()
        .enumerate()
        .map(|(i, t)| {
            decode_track_controlled(
                request.source,
                t.ordinal,
                start,
                end,
                control,
                10 + (i * 35 / selected.len().max(1)) as u8,
                10 + ((i + 1) * 35 / selected.len().max(1)) as u8,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let streams = if request.tracks.is_some() {
        remix_streams_controlled(decoded, &selected, request.output, control)?
    } else {
        decoded
    };
    if streams.iter().any(|(pcm, _)| pcm.is_empty()) {
        anyhow::bail!("une piste audio sélectionnée est vide dans cette plage");
    }

    let plan = plan_for(streams.len())?;
    let size = plan.map_or((crop.2, crop.3), |p| (p.width, p.height));
    let framing = (crop != (0, 0, src_w, src_h) || size != (src_w, src_h))
        .then_some(Framing { crop, size });
    let mut video_bps = match plan {
        Some(plan) => plan.video_bps,
        None => source_bps.map_or(12_000_000, |b| b as u32).max(300_000),
    };
    let mut attempt = 0;
    loop {
        let outcome = write_precise(
            &request,
            control,
            &index,
            (start, end),
            &streams,
            video_bps,
            framing,
        );
        // `write_precise` removes what it created; an earlier failure never
        // touched `dest`, which still belongs to whoever put it there.
        let outcome = outcome?;
        let Some(limit) = request.size_limit_bytes else { return Ok(outcome) };
        let written = std::fs::metadata(request.dest).context("taille de l’export")?.len();
        if written <= limit {
            return Ok(outcome);
        }
        if attempt == SIZE_RETRIES {
            discard(request.dest);
            anyhow::bail!(
                "impossible de faire tenir cette sélection dans {} Mo ; raccourcissez-la",
                limit / 1_000_000
            );
        }
        attempt += 1;
        match retry_bitrate(video_bps, written, limit, streams.len(), end - start) {
            Some(next) => video_bps = next,
            None => {
                discard(request.dest);
                anyhow::bail!(
                    "impossible de faire tenir cette sélection dans {} Mo ; raccourcissez-la",
                    limit / 1_000_000
                );
            }
        }
        tracing::info!(
            written,
            limit,
            video_bps,
            "size-capped export over the cap, encoding again"
        );
    }
}

/// The video bitrate for another encode after one landed at `written` bytes
/// over `limit`. Only the video share shrinks: the AAC streams are a fixed
/// cost. Aims 7 % under the cap; `None` when no lower bitrate is left to try.
fn retry_bitrate(
    video_bps: u32,
    written: u64,
    limit: u64,
    audio_streams: usize,
    duration_100ns: i64,
) -> Option<u32> {
    let audio = u128::from(AAC_BPS) * audio_streams as u128 * duration_100ns.max(0) as u128
        / 8
        / 10_000_000;
    let target = u128::from(limit) * 93 / 100;
    if target <= audio || u128::from(written) <= audio {
        return None;
    }
    let next = (u128::from(video_bps) * (target - audio) / (u128::from(written) - audio)) as u32;
    let next = next.max(MIN_VIDEO_BPS as u32);
    (next < video_bps).then_some(next)
}

/// One encode of the selected frames plus the prepared audio to `request.dest`.
pub(super) fn write_precise(
    request: &PreciseExport<'_>,
    control: &ExportControl,
    index: &FrameIndex,
    (start, end): (i64, i64),
    streams: &[(Vec<u8>, i64)],
    video_bps: u32,
    framing: Option<Framing>,
) -> Result<ExportOutcome> {
    let reader = open_compressed(request.source)?;
    let input = unsafe { MFCreateMediaType() }?;
    unsafe {
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(video_stream(), None, &input).context("décodage vidéo NV12")?;
    }
    let decoded_type = unsafe { reader.GetCurrentMediaType(video_stream()) }?;
    let layout = nv12_layout(&decoded_type)?;
    // Resizing happens here, frame by frame, rather than in the reader's video
    // processor: that one re-times a variable-rate source to its nominal rate,
    // losing the real frame times the precise cut is built on.
    let writer_input = match framing.map(|f| f.size) {
        None => decoded_type.clone(),
        Some((w, h)) => unsafe {
            let scaled = MFCreateMediaType()?;
            scaled.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            scaled.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            scaled.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
            scaled.SetUINT64(&MF_MT_FRAME_RATE, decoded_type.GetUINT64(&MF_MT_FRAME_RATE)?)?;
            scaled.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            scaled.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            scaled
        },
    };
    let frame_size = unsafe { writer_input.GetUINT64(&MF_MT_FRAME_SIZE) }?;
    let output = unsafe { MFCreateMediaType() }?;
    unsafe {
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        output.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
        output.SetUINT64(&MF_MT_FRAME_RATE, decoded_type.GetUINT64(&MF_MT_FRAME_RATE)?)?;
        output.SetUINT64(
            &MF_MT_PIXEL_ASPECT_RATIO,
            writer_input.GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO).unwrap_or((1u64 << 32) | 1),
        )?;
        output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, video_bps)?;
    }
    control.report(ExportStage::Writing, 50)?;
    // No destination ownership until the native writer creation is attempted.
    // Attribute setup can fail without ever touching the destination.
    let mut attrs = None;
    unsafe { MFCreateAttributes(&mut attrs, 1) }?;
    let attrs = attrs.context("attributs de l’encodeur")?;
    unsafe { attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1) }?;
    let result = (|| -> Result<ExportOutcome> {
        let writer =
            unsafe { MFCreateSinkWriterFromURL(&HSTRING::from(request.dest), None, Some(&attrs)) }
                .context("création de l’export précis")?;
        let video = unsafe { writer.AddStream(&output) }?;
        unsafe { writer.SetInputMediaType(video, &writer_input, None) }
            .context("entrée NV12 de l’encodeur")?;
        let audio =
            (0..streams.len()).map(|_| add_aac_stream(&writer)).collect::<Result<Vec<_>>>()?;
        unsafe { writer.BeginWriting() }?;
        let mut empty = 0;
        let mut packets = 0;
        let mut skipped = 0usize;
        let mut last_pts = None;
        loop {
            control.check()?;
            let mut flags = 0;
            let mut sample = None;
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
            .context("lecture vidéo pour la coupe précise")?;
            // A decoder fed a stream that opens before its first IDR hands back
            // placeholder samples with no picture; the encoder refuses those
            // (E_INVALIDARG), and there is nothing in them to keep.
            let sample = sample.filter(|sample| {
                let picture = unsafe { sample.GetTotalLength() }.unwrap_or(0) > 0;
                skipped += usize::from(!picture);
                picture
            });
            if let Some(sample) = sample {
                empty = 0;
                let decoded =
                    unsafe { sample.GetSampleTime() }.context("image décodée sans timestamp")?;
                // Decoders can round a timestamp a tick away from the stored
                // one, so match the index entry nearest the decoded time.
                let i = index.nearest_index(decoded).ok_or_else(|| {
                    anyhow::anyhow!("image décodée absente de l’index ({decoded})")
                })?;
                let pts = index.timestamps_100ns[i];
                if pts >= end {
                    break;
                }
                if pts >= start {
                    if last_pts.is_some_and(|last| pts <= last) {
                        anyhow::bail!("ordre des images décodées invalide");
                    }
                    let next = index
                        .timestamps_100ns
                        .get(i + 1)
                        .copied()
                        .unwrap_or(index.end_100ns)
                        .min(end);
                    let sample = match framing {
                        None => sample,
                        Some(framing) => resized_sample(&sample, layout, framing)?,
                    };
                    unsafe {
                        sample.SetSampleTime(pts - start)?;
                        sample.SetSampleDuration(next - pts)?;
                        writer
                            .WriteSample(video, &sample)
                            .context("encodage de l’image précise")?;
                    }
                    packets += 1;
                    last_pts = Some(pts);
                    control.report(ExportStage::Writing, media_percent(pts, start, end, 50, 80))?;
                }
            } else if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 == 0 && skipped == 0 {
                empty = count_empty_read(empty, request.source, "video")?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        if packets == 0 && skipped > 0 {
            anyhow::bail!(
                "Windows n’a pas pu décoder les images de ce clip (codec non pris en charge ?)"
            );
        }
        if packets == 0 {
            anyhow::bail!("cette plage ne contient aucune vidéo");
        }
        if skipped > 0 {
            tracing::info!(skipped, "precise export skipped decoder samples without a picture");
        }
        let second =
            crate::capture::audio::SAMPLE_RATE * crate::capture::audio::ENCODER_BLOCK_ALIGN;
        let total: usize = streams.iter().map(|(pcm, _)| pcm.len().div_ceil(second)).sum();
        let mut done = 0;
        for ((pcm, offset), stream) in streams.iter().zip(audio) {
            for (i, chunk) in pcm.chunks(second).enumerate() {
                control.check()?;
                write_pcm_sample(&writer, stream, offset + i as i64 * 10_000_000, chunk)?;
                done += 1;
                control.report(ExportStage::Writing, 80 + (done * 15 / total.max(1)) as u8)?;
            }
        }
        control.report(ExportStage::Finalizing, 95)?;
        unsafe { writer.Finalize() }.context("finalisation de l’export précis")?;
        control.report(ExportStage::Finalizing, 99)?;
        Ok(ExportOutcome {
            duration_ms: ((end - start) / 10_000) as u64,
            video_packets: packets,
            has_audio: !streams.is_empty(),
            frame_size: framing.map(|f| f.size),
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
    fn boundaries_use_actual_pts_and_ties_choose_the_previous_frame() {
        let index = FrameIndex {
            timestamps_100ns: (0..30).map(|i| i * 1_000_000).collect(),
            end_100ns: 30_000_000,
        };
        assert_eq!(index.range(550, 1550).unwrap(), (5_000_000, 15_000_000));
        let sixty =
            FrameIndex { timestamps_100ns: vec![0, 166_667, 333_333, 500_000], end_100ns: 666_667 };
        assert_eq!(sixty.range(17, 67).unwrap(), (166_667, 666_667));
    }
    #[test]
    fn nv12_scaling_halves_flat_planes_and_keeps_their_values() {
        // 4x4 luma of 200, then a 2-row UV plane of (90, 160) pairs.
        let mut src = vec![200u8; 16];
        src.extend([90, 160].repeat(4));
        let out = scale_nv12(&src, 4, 4, (0, 0, 4, 4), (2, 2)).unwrap();
        assert_eq!(out, vec![200, 200, 200, 200, 90, 160]);
    }

    #[test]
    fn nv12_scaling_reads_the_uv_plane_after_padded_rows() {
        // Luma 4x2 padded to 4 rows (rows 2..4 are garbage), then UV.
        let mut src = vec![50u8; 8];
        src.extend([255u8; 8]);
        src.extend([10, 20, 10, 20]);
        let out = scale_nv12(&src, 4, 4, (0, 0, 4, 2), (2, 2)).unwrap();
        assert_eq!(&out[..4], &[50, 50, 50, 50]);
        assert_eq!(&out[4..], &[10, 20]);
        assert!(scale_nv12(&src[..10], 4, 4, (0, 0, 4, 2), (2, 2)).is_err());
    }

    #[test]
    fn nv12_scaling_interpolates_a_gradient_monotonically() {
        let row: Vec<u8> = (0..8).map(|x| x * 30).collect();
        let mut src = row.repeat(2);
        src.extend([128u8; 8]);
        let out = scale_nv12(&src, 8, 2, (0, 0, 8, 2), (4, 2)).unwrap();
        let luma = &out[..4];
        assert!(luma.windows(2).all(|w| w[0] < w[1]), "{luma:?}");
        assert!(luma[0] >= 15 && luma[3] <= 195, "{luma:?}");
    }

    #[test]
    fn retry_shrinks_only_the_video_share_and_stops_when_nothing_is_left() {
        // 10 s, one AAC stream = 240 kB fixed. 10 MB written for an 8 MB cap.
        let next = retry_bitrate(7_000_000, 10_000_000, 8_000_000, 1, 100_000_000).unwrap();
        let expected = 7_000_000u128 * (7_440_000 - 240_000) / (10_000_000 - 240_000);
        assert_eq!(u128::from(next), expected);
        // Already at the floor: nothing lower to try.
        assert_eq!(retry_bitrate(150_000, 10_000_000, 8_000_000, 1, 100_000_000), None);
        // The audio alone fills the cap.
        assert_eq!(retry_bitrate(1_000_000, 300_000, 200_000, 1, 100_000_000), None);
    }

    #[test]
    fn nv12_scaling_honours_a_row_pitch_wider_than_the_picture() {
        // 4x2 luma in 8-byte rows (padding 99), then UV in the same pitch.
        let mut src = Vec::new();
        for _ in 0..2 {
            src.extend([70u8; 4]);
            src.extend([99u8; 4]);
        }
        src.extend([30, 40, 30, 40, 99, 99, 99, 99]);
        let out = scale_nv12(&src, 8, 2, (0, 0, 4, 2), (2, 2)).unwrap();
        assert_eq!(out, vec![70, 70, 70, 70, 30, 40]);
    }

    #[test]
    fn nv12_crop_keeps_only_the_chosen_columns_and_their_chroma() {
        // 8x2 luma: columns 0-3 = 10, 4-7 = 200. UV row: pairs (1,2),(3,4) | (5,6),(7,8).
        let mut src = Vec::new();
        for _ in 0..2 {
            src.extend([10u8; 4]);
            src.extend([200u8; 4]);
        }
        src.extend([1, 2, 3, 4, 5, 6, 7, 8]);
        let out = scale_nv12(&src, 8, 2, (4, 0, 4, 2), (4, 2)).unwrap();
        assert_eq!(&out[..8], &[200; 8]);
        assert_eq!(&out[8..], &[5, 6, 7, 8]);
        // Odd x would split a chroma pair; a crop past the pitch is refused.
        assert!(scale_nv12(&src, 8, 2, (3, 0, 4, 2), (4, 2)).is_err());
        assert!(scale_nv12(&src, 8, 2, (6, 0, 4, 2), (4, 2)).is_err());
    }

    #[test]
    fn vertical_crop_is_nine_by_sixteen_even_and_stays_inside_the_picture() {
        assert_eq!(vertical_crop(1920, 1080, 500), (656, 0, 606, 1080));
        assert_eq!(vertical_crop(1920, 1080, 0), (0, 0, 606, 1080));
        assert_eq!(vertical_crop(1920, 1080, 1000), (1314, 0, 606, 1080));
        assert_eq!(vertical_crop(2560, 1440, 500), (874, 0, 810, 1440));
        // Already portrait: kept whole.
        assert_eq!(vertical_crop(1080, 1920, 500), (0, 0, 1080, 1920));
        for c in [0, 123, 500, 777, 1000, 4000] {
            let (x, y, w, h) = vertical_crop(1366, 768, c);
            assert!(x % 2 == 0 && w % 2 == 0 && x + w <= 1366 && y == 0 && h == 768);
        }
    }

    #[test]
    fn decoded_times_match_the_nearest_stored_frame_within_a_millisecond() {
        let index =
            FrameIndex { timestamps_100ns: vec![0, 333_333, 666_666], end_100ns: 1_000_000 };
        assert_eq!(index.nearest_index(666_667), Some(2));
        assert_eq!(index.nearest_index(333_332), Some(1));
        assert_eq!(index.nearest_index(0), Some(0));
        assert_eq!(index.nearest_index(500_000), None);
        assert_eq!(index.nearest_index(2_000_000), None);
    }

    #[test]
    fn share_plan_keeps_the_source_size_when_the_budget_allows_it() {
        // 30 s into 50 MB: ~11.8 Mb/s of video, plenty for 1080p60.
        let plan = share_plan(1920, 1080, 60_000, 300_000_000, 1, 50_000_000, None).unwrap();
        assert_eq!((plan.width, plan.height), (1920, 1080));
        assert_eq!(plan.video_bps, 50_000_000 * 8 * 9 / 10 / 30 - 192_000);
    }

    #[test]
    fn share_plan_steps_down_to_a_size_the_bitrate_can_carry() {
        // 30 s into 20 MB: ~4.6 Mb/s, under 0.04 bpp at 1080p60 -> 720p.
        let plan = share_plan(1920, 1080, 60_000, 300_000_000, 1, 20_000_000, None).unwrap();
        assert_eq!((plan.width, plan.height), (1280, 720));
        // Portrait sources keep their orientation and even dimensions.
        let tall = share_plan(1080, 1920, 60_000, 300_000_000, 1, 20_000_000, None).unwrap();
        assert_eq!((tall.width, tall.height), (720, 1280));
        // Ultrawide keeps its aspect with an even long side.
        let wide = share_plan(3440, 1440, 60_000, 600_000_000, 1, 20_000_000, None).unwrap();
        assert_eq!(wide.width % 2, 0);
        assert!(wide.height < 1440);
    }

    #[test]
    fn share_plan_never_exceeds_the_source_bitrate_or_upscales() {
        let plan =
            share_plan(1280, 720, 30_000, 50_000_000, 1, 500_000_000, Some(8_000_000)).unwrap();
        assert_eq!(plan.video_bps, 8_000_000);
        assert_eq!((plan.width, plan.height), (1280, 720));
        let small = share_plan(640, 360, 60_000, 600_000_000, 1, 10_000_000, None).unwrap();
        assert!(small.width <= 640 && small.height <= 360);
    }

    #[test]
    fn share_plan_refuses_a_cap_too_small_for_the_duration() {
        // 10 min into 10 MB is ~133 kb/s total: below the audio plus floor.
        let error = share_plan(1920, 1080, 60_000, 6_000_000_000, 1, 10_000_000, None).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("10 Mo") && text.contains("raccourcissez"), "{text}");
    }

    #[test]
    fn pcm_selection_trims_both_edges_inside_a_packet() {
        assert_eq!(pcm_frame_range(1_000_000, 1_100_000, 1_200_000, 4800 * 4), (480, 960));
        assert_eq!(pcm_frame_range(0, 1, 209, 4 * 8), (1, 2));
        assert_eq!(
            pcm_frame_range_on_grid(0, 4_906_666, 5_000_000, 15_000_000, 1024 * 4),
            (448, 1024)
        );
        assert_eq!(
            pcm_frame_range_on_grid(0, 4_906_667, 5_000_000, 15_000_000, 1024 * 4),
            (448, 1024)
        );
    }
}
