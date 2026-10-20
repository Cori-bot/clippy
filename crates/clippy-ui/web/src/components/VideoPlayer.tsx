import { Button } from '@openai/apps-sdk-ui/components/Button';
import {
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type Ref,
} from 'react';
import { formatClock } from '../lib/timeline';
import { clamp, pointerRatio, ratioToValue, stepBy, valueToRatio } from '../lib/ui';
import {
  VOLUME_STEP,
  VOLUME_STORAGE_KEY,
  nudgeVolume,
  parseStoredVolume,
  unmutedLevel,
} from '../lib/volume';
import { Fullscreen, FullscreenExit, Pause, Play, Volume, VolumeMute } from './icons';

/** What `ClipPage` can ask of the player from outside. */
export type VideoPlayerHandle = {
  toggle: () => void;
  seekTo: (ms: number) => void;
  stepVolume: (direction: 1 | -1) => void;
};

function readStoredLevel(): number {
  try {
    return parseStoredVolume(localStorage.getItem(VOLUME_STORAGE_KEY));
  } catch {
    return 100;
  }
}

/**
 * Upright volume slider filling from the bottom. The SDK's Slider is
 * horizontal only; this keeps the original's keys (arrows, PageUp/Down, Home,
 * End), which it stops from reaching the page's own Up/Down volume shortcut.
 */
function VerticalSlider({
  value,
  label,
  onChange,
}: {
  value: number;
  label: string;
  onChange: (v: number) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const ratio = valueToRatio(value, 0, 100);

  const valueAt = (e: ReactPointerEvent) => {
    const el = track.current;
    if (!el) return value;
    return ratioToValue(pointerRatio(el.getBoundingClientRect(), e.clientX, e.clientY, true), 0, 100, VOLUME_STEP);
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    const delta =
      e.key === 'ArrowRight' || e.key === 'ArrowUp' ? 1
      : e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -1
      : e.key === 'PageUp' ? 10
      : e.key === 'PageDown' ? -10
      : 0;
    let next: number;
    if (delta !== 0) next = stepBy(value, delta, 0, 100, VOLUME_STEP);
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = 100;
    else return;
    e.preventDefault();
    e.stopPropagation();
    onChange(next);
  };

  return (
    <div
      ref={track}
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-orientation="vertical"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamp(value, 0, 100)}
      className="group/vs relative flex h-24 w-6 cursor-pointer touch-none justify-center rounded-md outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-border-strong)]"
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        setDragging(true);
        onChange(valueAt(e));
      }}
      onPointerMove={(e) => {
        if (dragging) onChange(valueAt(e));
      }}
      onPointerUp={(e) => {
        if (!dragging) return;
        setDragging(false);
        e.currentTarget.releasePointerCapture(e.pointerId);
        onChange(valueAt(e));
      }}
      onKeyDown={onKeyDown}>
      <span className="absolute inset-y-0 w-1 rounded-full bg-[var(--color-border-strong)]" />
      <span
        className="absolute bottom-0 w-1 rounded-full bg-[var(--color-background-primary-solid)]"
        style={{ height: `${ratio * 100}%` }}
      />
      <span
        className="absolute size-3.5 rounded-full border-2 border-[var(--color-surface-elevated,var(--color-text))] bg-[var(--color-background-primary-solid)] shadow-sm transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] group-hover/vs:scale-110"
        style={{ bottom: `calc(${ratio * 100}% - 7px)` }}
      />
    </div>
  );
}

export function VideoPlayer({
  src,
  onTime,
  timeline,
  ref,
}: {
  src: string;
  onTime: (ms: number) => void;
  /**
   * The unified band, rendered inside the transport row. Handed the duration
   * this player read off the file -- the only duration available for a clip
   * the library recorded as `duration_ms: 0`, and 0 until `loadedmetadata`.
   */
  timeline: (durationMs: number) => ReactNode;
  ref?: Ref<VideoPlayerHandle>;
}) {
  const video = useRef<HTMLVideoElement>(null);
  /** The wrapper, not the video: fullscreen on the `<video>` alone would take
      the transport row -- its sibling -- off screen. */
  const stage = useRef<HTMLDivElement>(null);
  const volEl = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [full, setFull] = useState(false);
  const [positionMs, setPositionMs] = useState(0);
  const [durationMs, setDurationMs] = useState(0);

  // Volume. The level is remembered across launches; muting is not, so a clip
  // never opens silent because of something done in an earlier session.
  const [level, setLevelState] = useState(readStoredLevel);
  const lastAudible = useRef(level > 0 ? level : 100);

  // The popup is open while the pointer is over the speaker or the popup,
  // while keyboard focus is in either, and for a moment after a key or wheel
  // step. Not on mouse focus: WebView2 focuses a button when it is clicked, and
  // a popup that stayed up after clicking mute would read as stuck.
  const [hovering, setHovering] = useState(false);
  const [keyboardFocus, setKeyboardFocus] = useState(false);
  const [flashing, setFlashing] = useState(false);
  const volumeOpen = hovering || keyboardFocus || flashing;
  const leaveTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const flashTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  // `volume` and `muted` belong to the element and survive `src` changing, but
  // they are set from state all the same, so the speaker icon and what comes
  // out of the speakers cannot disagree.
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    v.volume = level / 100;
    v.muted = muted;
  }, [level, muted, src]);

  useEffect(() => {
    try {
      localStorage.setItem(VOLUME_STORAGE_KEY, String(level));
    } catch {
      // Storage refused (a blocked profile): the level lasts this session only.
    }
  }, [level]);

  const setLevel = useCallback((v: number) => {
    setLevelState(v);
    if (v > 0) {
      lastAudible.current = v;
      setMuted(false);
    }
  }, []);

  const levelRef = useRef(level);
  levelRef.current = level;

  const stepVolume = useCallback(
    (direction: 1 | -1) => {
      if (direction === 1) setMuted(false);
      setLevel(nudgeVolume(levelRef.current, direction));
      setFlashing(true);
      clearTimeout(flashTimer.current);
      flashTimer.current = setTimeout(() => setFlashing(false), 1000);
    },
    [setLevel],
  );

  const toggle = useCallback(() => {
    const v = video.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  }, []);

  const seekTo = useCallback((ms: number) => {
    if (video.current) video.current.currentTime = ms / 1000;
  }, []);

  useImperativeHandle(ref, () => ({ toggle, seekTo, stepVolume }), [toggle, seekTo, stepVolume]);

  const toggleMute = () => {
    if (muted || level === 0) {
      setLevel(unmutedLevel(level, lastAudible.current));
      setMuted(false);
    } else {
      setMuted(true);
    }
  };

  // The wheel over the speaker or its popup steps the volume. Registered by
  // hand because it has to call `preventDefault` -- the clip page scrolls --
  // and React attaches `onWheel` as a passive listener, which cannot.
  useEffect(() => {
    const el = volEl.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0) return;
      e.preventDefault();
      stepVolume(e.deltaY < 0 ? 1 : -1);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [stepVolume]);

  useEffect(
    () => () => {
      clearTimeout(leaveTimer.current);
      clearTimeout(flashTimer.current);
    },
    [],
  );

  // Cleared when the source changes, because nothing else does it: `ClipPage`
  // repoints `src` on the same instance, and `loadedmetadata` skips the
  // assignment for a non-finite duration -- so an unfinalized mp4 stepped to
  // from a 60s clip would keep reading `/ 1:00.0`. Zero is the honest value.
  useEffect(() => {
    setDurationMs(0);
    setPositionMs(0);
  }, [src]);

  useEffect(() => {
    const onChange = () => setFull(document.fullscreenElement === stage.current);
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  const toggleFull = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void stage.current?.requestFullscreen();
  };

  const onVolEnter = () => {
    clearTimeout(leaveTimer.current);
    setHovering(true);
  };
  // A short grace period, so crossing from the speaker up to the popup -- or
  // overshooting its edge mid-adjustment -- does not close it.
  const onVolLeave = () => {
    clearTimeout(leaveTimer.current);
    leaveTimer.current = setTimeout(() => setHovering(false), 250);
  };

  const silent = muted || level === 0;

  return (
    <div
      ref={stage}
      className="grid gap-3 [&:fullscreen]:content-center [&:fullscreen]:bg-black [&:fullscreen]:px-6 [&:fullscreen]:pb-6">
      <video
        ref={video}
        src={src}
        autoPlay
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onLoadedMetadata={() => {
          // Guarded, not just truthy-checked: an unfinalized or fragmented mp4
          // reports `Infinity`, which sails past every `durationMs <= 0` guard
          // in timeline.ts and lands in `currentTime`, whose setter rejects a
          // non-finite double.
          const v = video.current;
          if (v && Number.isFinite(v.duration)) setDurationMs(v.duration * 1000);
        }}
        onTimeUpdate={() => {
          const v = video.current;
          if (!v) return;
          const ms = v.currentTime * 1000;
          setPositionMs(ms);
          onTime(ms);
        }}
        onClick={toggle}
        className="block max-h-[58vh] w-full cursor-pointer rounded-xl bg-black [:fullscreen_&]:max-h-[calc(100vh-100px)]"
      />

      <div className="flex items-center gap-3">
        <Button
          color="primary"
          size="lg"
          uniform
          aria-label={playing ? 'Pause' : 'Lecture'}
          className="shrink-0 transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]"
          onClick={toggle}>
          {playing ? <Pause className="size-4" /> : <Play className="size-4" />}
        </Button>

        <div className="min-w-0 flex-1">{timeline(durationMs)}</div>

        <span className="shrink-0 text-xs text-tertiary tabular-nums">
          {formatClock(positionMs)} / {formatClock(durationMs)}
        </span>

        {/* The popup comes after the speaker in the markup so Tab reaches the
            speaker first and then the slider, though it is drawn above. */}
        <div
          ref={volEl}
          role="group"
          aria-label="Volume"
          className="relative flex"
          onPointerEnter={onVolEnter}
          onPointerLeave={onVolLeave}
          onFocus={(e) => {
            const el = e.target as Element;
            setKeyboardFocus(typeof el.matches === 'function' && el.matches(':focus-visible'));
          }}
          onBlur={(e) => {
            if (!(e.relatedTarget instanceof Node && volEl.current?.contains(e.relatedTarget))) {
              setKeyboardFocus(false);
            }
          }}>
          <Button
            color="secondary"
            variant="ghost"
            size="sm"
            uniform
            aria-label={silent ? 'Réactiver le son' : 'Couper le son'}
            className="transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]"
            onClick={toggleMute}>
            {silent ? <VolumeMute className="size-4" /> : <Volume className="size-4" />}
          </Button>
          {/* Above the speaker, over the bottom of the video, so opening it
              moves nothing. The padding is the bridge the pointer crosses from
              the speaker, so the gap does not count as leaving. */}
          {volumeOpen && (
            <div className="absolute bottom-full left-1/2 z-10 -translate-x-1/2 pb-1.5">
              <div className="grid animate-[pop-in_150ms_cubic-bezier(0.22,1,0.36,1)] justify-items-center gap-2 rounded-lg border border-default bg-surface-elevated px-1.5 pt-2 pb-3 shadow-lg">
                <span className="text-xs text-secondary tabular-nums">{muted ? 0 : level}</span>
                <VerticalSlider label="Volume" value={muted ? 0 : level} onChange={setLevel} />
              </div>
            </div>
          )}
        </div>

        <Button
          color="secondary"
          variant="ghost"
          size="sm"
          uniform
          aria-label={full ? 'Quitter le plein écran' : 'Plein écran'}
          className="transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]"
          onClick={toggleFull}>
          {full ? <FullscreenExit className="size-4" /> : <Fullscreen className="size-4" />}
        </Button>
      </div>
    </div>
  );
}
