import { useEffect, useRef, useState, type RefObject } from 'react';
import { needsResync } from './tracks';

/**
 * Plays one hidden `<audio>` per source track in lockstep with a muted
 * `<video>`, which is the master clock.
 *
 * - play waits until every element can play through (the video is paused and
 *   resumed by this hook), so tracks never start at different moments;
 * - pause, seeks, rate changes and stalls are mirrored onto every element;
 * - while playing, a requestAnimationFrame check re-syncs any element whose
 *   time is more than `DRIFT_LIMIT_S` away from the video's;
 * - `volumes` (0..1 per track, master already applied) is written to the
 *   elements as it changes.
 *
 * Returns `active`: true while the separate tracks drive the sound, so the
 * caller keeps the video muted. If a file fails to load it turns false and the
 * video's own (mixed) audio is used instead, rather than leaving the clip mute.
 */
export function useTrackSync(
  video: RefObject<HTMLVideoElement | null>,
  urls: string[],
  volumes: number[],
): { active: boolean } {
  const [failed, setFailed] = useState(false);
  const audiosRef = useRef<HTMLAudioElement[]>([]);
  const volumesRef = useRef(volumes);
  volumesRef.current = volumes;
  const key = urls.join('\n');

  useEffect(() => {
    setFailed(false);
    const v = video.current;
    if (!v || urls.length === 0) return;

    const audios = urls.map((url) => {
      const a = new Audio();
      a.preload = 'auto';
      a.src = url;
      return a;
    });
    audiosRef.current = audios;
    audios.forEach((a, i) => (a.volume = volumesRef.current[i] ?? 0));

    let pending = false; // video held back until every audio can play through
    let raf = 0;

    const allReady = () => audios.every((a) => a.readyState >= 3);
    const align = () => {
      for (const a of audios) {
        if (a.playbackRate !== v.playbackRate) a.playbackRate = v.playbackRate;
        if (needsResync(a.currentTime, v.currentTime)) a.currentTime = v.currentTime;
      }
    };
    const setTimes = () => {
      for (const a of audios) a.currentTime = v.currentTime;
    };
    const playAll = () => {
      for (const a of audios) if (a.paused) a.play().catch(() => {});
    };
    const pauseAll = () => audios.forEach((a) => a.pause());

    const loop = () => {
      if (!v.paused && !v.seeking) align();
      raf = requestAnimationFrame(loop);
    };
    const stopLoop = () => {
      cancelAnimationFrame(raf);
      raf = 0;
    };

    const onPlay = () => {
      if (!allReady()) {
        pending = true;
        v.pause();
        return;
      }
      setTimes();
      audios.forEach((a) => (a.playbackRate = v.playbackRate));
      playAll();
      if (!raf) raf = requestAnimationFrame(loop);
    };
    const onPause = () => {
      stopLoop();
      pauseAll();
      if (!pending) setTimes();
    };
    const onSeeking = () => setTimes();
    const onSeeked = () => {
      setTimes();
      if (!v.paused && allReady()) playAll();
    };
    const onWaiting = () => pauseAll();
    const onPlaying = () => {
      if (!allReady()) return;
      setTimes();
      playAll();
      if (!raf) raf = requestAnimationFrame(loop);
    };
    const onRate = () => align();
    const onEnded = () => {
      stopLoop();
      pauseAll();
    };
    const onCanPlay = () => {
      if (!allReady()) return;
      if (pending) {
        pending = false;
        void v.play().catch(() => {});
        return;
      }
      // The video was already playing (it started on its mixed audio while the
      // tracks loaded): join it.
      if (!v.paused && !v.seeking) {
        align();
        playAll();
        if (!raf) raf = requestAnimationFrame(loop);
      }
    };
    const onError = () => {
      pending = false;
      stopLoop();
      pauseAll();
      setFailed(true);
      if (v.paused) void v.play().catch(() => {});
    };

    const events: [string, () => void][] = [
      ['play', onPlay],
      ['pause', onPause],
      ['seeking', onSeeking],
      ['seeked', onSeeked],
      ['waiting', onWaiting],
      ['playing', onPlaying],
      ['ratechange', onRate],
      ['ended', onEnded],
    ];
    events.forEach(([n, h]) => v.addEventListener(n, h));
    audios.forEach((a) => {
      a.addEventListener('canplaythrough', onCanPlay);
      a.addEventListener('canplay', onCanPlay);
      a.addEventListener('error', onError);
    });

    // Already playing when the tracks arrive: join, once they can play through.
    onCanPlay();

    return () => {
      events.forEach(([n, h]) => v.removeEventListener(n, h));
      stopLoop();
      audios.forEach((a) => {
        a.removeEventListener('canplaythrough', onCanPlay);
        a.removeEventListener('canplay', onCanPlay);
        a.removeEventListener('error', onError);
        a.pause();
        a.removeAttribute('src');
        a.load();
      });
      audiosRef.current = [];
      // Do not leave the video held back by a pending start.
      if (pending && v.paused) void v.play().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, video]);

  const volKey = volumes.join(',');
  useEffect(() => {
    audiosRef.current.forEach((a, i) => (a.volume = volumes[i] ?? 0));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [volKey, key]);

  return { active: urls.length > 0 && !failed };
}
