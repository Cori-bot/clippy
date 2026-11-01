import { useEffect, useRef, useState, type RefObject } from 'react';
import { needsResync } from './tracks';

/**
 * Synchronizes source tracks against the video clock. A muted Web Audio bus
 * keeps the mixed video audible until the context is running, every source is
 * buffered, and all play() calls succeed. GainNodes preserve export's 0..200%.
 */
export function useTrackSync(
  video: RefObject<HTMLVideoElement | null>,
  urls: string[],
  volumes: number[],
): { active: boolean } {
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const gainsRef = useRef<GainNode[]>([]);
  const volumesRef = useRef(volumes);
  volumesRef.current = volumes;
  const key = urls.join('\n');

  useEffect(() => {
    setActiveKey(null);
    const v = video.current;
    if (!v || urls.length === 0) return;

    let context: AudioContext;
    try {
      context = new AudioContext();
    } catch {
      return; // Keep the mixed video audible when Web Audio is unavailable.
    }
    const audios = urls.map((url) => {
      const a = new Audio();
      a.preload = 'auto';
      // Tauri's asset protocol supplies Access-Control-Allow-Origin.
      a.crossOrigin = 'anonymous';
      a.src = url;
      return a;
    });
    const sources: MediaElementAudioSourceNode[] = [];
    const gains: GainNode[] = [];
    const output = context.createGain();
    output.gain.value = 0;
    output.connect(context.destination);
    try {
      audios.forEach((a, i) => {
        const source = context.createMediaElementSource(a);
        sources.push(source);
        const gain = context.createGain();
        gains.push(gain);
        gain.gain.value = volumesRef.current[i] ?? 0;
        source.connect(gain);
        gain.connect(output);
      });
    } catch {
      sources.forEach((source) => source.disconnect());
      gains.forEach((gain) => gain.disconnect());
      output.disconnect();
      audios.forEach((a) => { a.removeAttribute('src'); a.load(); });
      void context.close().catch(() => {});
      return;
    }
    gainsRef.current = gains;

    let raf = 0;
    let disposed = false;
    let broken = false;
    let starting = false;
    let generation = 0;
    const allReady = () => audios.every((a) => a.readyState >= 3);
    const align = (force = false) => {
      for (const a of audios) {
        a.playbackRate = v.playbackRate;
        if (force || needsResync(a.currentTime, v.currentTime)) a.currentTime = v.currentTime;
      }
    };
    const loop = () => {
      if (!v.paused && !v.seeking) align();
      raf = requestAnimationFrame(loop);
    };
    const pauseAll = () => {
      generation++;
      starting = false;
      cancelAnimationFrame(raf);
      raf = 0;
      audios.forEach((a) => a.pause());
    };
    const deactivate = () => {
      output.gain.value = 0;
      setActiveKey(null);
    };
    const onError = () => {
      if (disposed || broken) return;
      broken = true;
      deactivate();
      pauseAll();
    };
    const start = () => {
      if (disposed || broken || starting || v.paused || v.seeking || !allReady()
        || context.state !== 'running') return;
      starting = true;
      const attempt = ++generation;
      align(true);
      // A pending resume() or play() must never silence the mixed preview.
      void Promise.all(audios.map((a) => a.paused ? a.play() : Promise.resolve())).then(() => {
        if (disposed || broken || attempt !== generation) return;
        starting = false;
        if (v.paused || v.seeking || context.state !== 'running' || !allReady()) return;
        align();
        // Switch both outputs together, before React commits active state.
        v.muted = true;
        output.gain.value = 1;
        setActiveKey(key);
        if (!raf) raf = requestAnimationFrame(loop);
      }).catch((error: unknown) => {
        if (disposed || attempt !== generation) return;
        starting = false;
        if (!(error instanceof DOMException && error.name === 'AbortError')) onError();
      });
    };
    const onPlay = () => {
      if (disposed || broken) return;
      // Unlock during user activation; resume may stay pending under autoplay
      // policy, so only statechange/running can authorize the output switch.
      if (context.state === 'suspended') void context.resume().then(start).catch(onError);
      start();
    };
    const onState = () => {
      if (disposed || broken) return;
      if (context.state === 'running') start();
      else { deactivate(); pauseAll(); }
    };
    const onPause = () => { pauseAll(); align(true); };
    const onSeeking = () => { pauseAll(); align(true); };
    const onWaiting = () => { deactivate(); pauseAll(); };
    const onAudioWaiting = () => { deactivate(); pauseAll(); };
    const events: [string, () => void][] = [
      ['play', onPlay], ['pause', onPause], ['seeking', onSeeking],
      ['seeked', start], ['waiting', onWaiting], ['playing', start],
      ['ratechange', align], ['ended', pauseAll],
    ];
    events.forEach(([n, h]) => v.addEventListener(n, h));
    context.addEventListener('statechange', onState);
    audios.forEach((a) => {
      a.addEventListener('canplaythrough', start);
      a.addEventListener('canplay', start);
      a.addEventListener('waiting', onAudioWaiting);
      a.addEventListener('error', onError);
    });
    onPlay();

    return () => {
      disposed = true;
      events.forEach(([n, h]) => v.removeEventListener(n, h));
      context.removeEventListener('statechange', onState);
      output.gain.value = 0;
      pauseAll();
      audios.forEach((a) => {
        a.removeEventListener('canplaythrough', start);
        a.removeEventListener('canplay', start);
        a.removeEventListener('waiting', onAudioWaiting);
        a.removeEventListener('error', onError);
        a.removeAttribute('src');
        a.load();
      });
      gainsRef.current = [];
      sources.forEach((source) => source.disconnect());
      gains.forEach((gain) => gain.disconnect());
      output.disconnect();
      void context.close().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, video]);

  const volKey = volumes.join(',');
  useEffect(() => {
    gainsRef.current.forEach((gain, i) => (gain.gain.value = volumes[i] ?? 0));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [volKey, key]);

  return { active: urls.length > 0 && activeKey === key };
}
