import { useEffect, useState } from 'react';
import { diag } from '../lib/diag';
import { MAX_WAVEFORM_DURATION_MS, readBoundedAudio, videoFilmstrip, waveformPeaks } from '../lib/media-previews';

export function Filmstrip({ src }: { src?: string }) {
  const [result, setResult] = useState<{ src: string; frames: string[]; failed: boolean }>();
  useEffect(() => {
    if (!src) return;
    const controller = new AbortController();
    void videoFilmstrip(src, controller.signal).then(
      (frames) => { if (!controller.signal.aborted) setResult({ src, frames, failed: false }); },
      () => { if (!controller.signal.aborted) setResult({ src, frames: [], failed: true }); },
    );
    return () => controller.abort();
  }, [src]);
  const current = result?.src === src ? result : undefined;
  return <div className="timeline-filmstrip" aria-hidden="true">
    {current?.frames.length
      ? current.frames.map((frame, i) => <img key={i} src={frame} alt="" draggable={false} />)
      : <span className="timeline-preview-note">{current?.failed || !src ? 'Aperçu vidéo indisponible' : 'Chargement des images…'}</span>}
  </div>;
}

export function Waveform({ src, durationMs }: { src: string; durationMs: number }) {
  const [result, setResult] = useState<{ key: string; peaks: number[]; failed: boolean }>();
  const key = `${src}\n${durationMs > MAX_WAVEFORM_DURATION_MS}`;
  const tooLong = durationMs > MAX_WAVEFORM_DURATION_MS;
  useEffect(() => {
    if (tooLong) return;
    const controller = new AbortController();
    // Offline decoding neither opens an audio output nor competes with the mixer.
    let context: OfflineAudioContext | undefined;
    void (async () => {
      const response = await fetch(src, { signal: controller.signal });
      const bytes = await readBoundedAudio(response, controller.signal);
      controller.signal.throwIfAborted();
      context = new OfflineAudioContext(1, 1, 16_000);
      const audio = await context.decodeAudioData(bytes);
      controller.signal.throwIfAborted();
      const peaks = waveformPeaks(Array.from({ length: audio.numberOfChannels }, (_, channel) => audio.getChannelData(channel)));
      setResult({ key, peaks, failed: false });
    })().catch((error: unknown) => {
      if (controller.signal.aborted) return;
      diag('waveform-error', { src: src.slice(-80), error: String(error) });
      setResult({ key, peaks: [], failed: true });
    });
    return () => { controller.abort(); context = undefined; };
  }, [src, key, tooLong]);
  const current = result?.key === key ? result : undefined;
  const peaks = current?.peaks ?? [];
  return <div className="timeline-waveform" aria-hidden="true">
    {peaks.length > 0 ? <svg viewBox={`0 0 ${peaks.length} 32`} preserveAspectRatio="none">
      <path d={peaks.map((peak, i) => `M${i + 0.5},${16 - peak * 14}v${Math.max(0.5, peak * 28)}`).join(' ')} />
    </svg> : <span className="timeline-preview-note">{tooLong ? 'Forme d’onde disponible pour les clips de moins de 2 min' : current?.failed ? 'Forme d’onde indisponible' : 'Analyse audio…'}</span>}
  </div>;
}
