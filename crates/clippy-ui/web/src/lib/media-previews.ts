/** Visual previews are disposable derivatives, never playback or export inputs. */
export const MAX_WAVEFORM_BYTES = 16 * 1024 * 1024;
export const MAX_WAVEFORM_DURATION_MS = 120_000;

export function waveformPeaks(channels: readonly Float32Array[], count = 640): number[] {
  const length = channels.reduce((max, channel) => Math.max(max, channel.length), 0);
  if (!length || count <= 0) return [];
  const bins = Math.min(length, Math.floor(count));
  return Array.from({ length: bins }, (_, bin) => {
    const start = Math.floor(bin * length / bins);
    const end = Math.floor((bin + 1) * length / bins);
    let peak = 0;
    for (const channel of channels) {
      for (let i = start; i < Math.min(end, channel.length); i++) {
        const sample = Math.abs(channel[i]);
        if (Number.isFinite(sample)) peak = Math.max(peak, sample);
      }
    }
    return Math.min(1, peak);
  });
}

export async function readBoundedAudio(response: Response, signal: AbortSignal): Promise<ArrayBuffer> {
  if (!response.ok) throw new Error('La piste audio est inaccessible.');
  if (Number(response.headers.get('content-length')) > MAX_WAVEFORM_BYTES) {
    await response.body?.cancel();
    throw new Error('Piste trop volumineuse pour afficher sa forme d’onde.');
  }
  const reader = response.body?.getReader();
  if (!reader) throw new Error('La piste audio est inaccessible.');
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_WAVEFORM_BYTES) throw new Error('Piste trop volumineuse pour afficher sa forme d’onde.');
      chunks.push(value);
    }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  const buffer = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.byteLength; }
  return buffer.buffer;
}

function mediaEvent(video: HTMLVideoElement, event: 'loadeddata' | 'seeked', signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const clean = () => {
      clearTimeout(timer);
      video.removeEventListener(event, ready);
      video.removeEventListener('error', failed);
      signal.removeEventListener('abort', aborted);
    };
    const ready = () => { clean(); resolve(); };
    const failed = () => { clean(); reject(new Error('Aperçu vidéo indisponible.')); };
    const aborted = () => { clean(); reject(new DOMException('Aborted', 'AbortError')); };
    const timer = setTimeout(failed, 10_000);
    video.addEventListener(event, ready, { once: true });
    video.addEventListener('error', failed, { once: true });
    signal.addEventListener('abort', aborted, { once: true });
    if (signal.aborted) aborted();
  });
}

/** A separate muted decoder ensures thumbnail seeks never disturb the player. */
export async function videoFilmstrip(src: string, signal: AbortSignal): Promise<string[]> {
  const video = document.createElement('video');
  video.muted = true;
  video.preload = 'auto';
  video.crossOrigin = 'anonymous';
  try {
    const loaded = mediaEvent(video, 'loadeddata', signal);
    video.src = src;
    video.load();
    await loaded;
    if (!Number.isFinite(video.duration) || video.duration <= 0) throw new Error('Durée vidéo indisponible.');
    const canvas = document.createElement('canvas');
    canvas.width = 160;
    canvas.height = 90;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('Aperçu vidéo indisponible.');
    const frames: string[] = [];
    for (let i = 0; i < 12; i++) {
      signal.throwIfAborted();
      const time = Math.min(video.duration - 0.001, (i + 0.5) * video.duration / 12);
      if (Math.abs(time - video.currentTime) > 0.0001) {
        const sought = mediaEvent(video, 'seeked', signal);
        video.currentTime = Math.max(0, time);
        await sought;
      }
      const scale = Math.max(canvas.width / video.videoWidth, canvas.height / video.videoHeight);
      const width = video.videoWidth * scale;
      const height = video.videoHeight * scale;
      ctx.drawImage(video, (canvas.width - width) / 2, (canvas.height - height) / 2, width, height);
      frames.push(canvas.toDataURL('image/jpeg', 0.65));
    }
    return frames;
  } finally {
    video.removeAttribute('src');
    video.load();
  }
}
