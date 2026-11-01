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
import { formatClock, formatPreciseClock, frameStep, nearestFrameBoundary, presentedFrameBoundary, type FrameIndex } from '../lib/timeline';
import { trackVolume, type TrackState } from '../lib/tracks';
import { useTrackSync } from '../lib/useTrackSync';
import { verticalBand } from '../lib/share';
import { clamp, pointerRatio, ratioToValue, stepBy, valueToRatio } from '../lib/ui';
import {
  VOLUME_STEP,
  VOLUME_STORAGE_KEY,
  nudgeVolume,
  parseStoredVolume,
  unmutedLevel,
} from '../lib/volume';
import { ChevronLeft, ChevronRight, Fullscreen, FullscreenExit, Pause, Play, Volume, VolumeMute } from './icons';
import './pro-player.css';

/** What `ClipPage` can ask of the player from outside. */
export type VideoPlayerHandle = {
  toggle: () => void;
  seekTo: (ms: number) => void;
  seekFrame: (ms: number) => void;
  stepFrame: (direction: 1 | -1) => void;
  /** Read synchronously for In/Out, independently of throttled timeupdate. */
  currentFrame: () => number;
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
  inspector,
  trackUrls = [],
  trackStates = [],
  frameIndex,
  verticalCenter = null,
  onVerticalCenter,
  ref,
}: {
  src: string;
  /** Shows the 9:16 crop centred this many thousandths across; null hides it. */
  verticalCenter?: number | null;
  onVerticalCenter?: (center: number) => void;
  frameIndex?: FrameIndex;
  /**
   * Separate audio files, one per editable track. When given, the `<video>`
   * is muted and these play in sync instead; absent or empty keeps the
   * video's own audio (old clips, single-stream clips, tracks still loading).
   */
  trackUrls?: string[];
  trackStates?: TrackState[];
  onTime: (ms: number) => void;
  inspector?: ReactNode;
  /**
   * The full-width editing timeline below the transport. Handed the duration
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
  /** Where the picture is drawn inside the viewer (object-fit: contain). */
  const [frame, setFrame] = useState<PictureFrame | null>(null);
  const measureFrame = useCallback(() => {
    const v = video.current;
    if (!v || !v.videoWidth || !v.videoHeight) return setFrame(null);
    const scale = Math.min(v.clientWidth / v.videoWidth, v.clientHeight / v.videoHeight);
    const width = v.videoWidth * scale;
    const height = v.videoHeight * scale;
    setFrame({ left: v.offsetLeft + (v.clientWidth - width) / 2, top: v.offsetTop + (v.clientHeight - height) / 2, width, height });
  }, []);
  useEffect(() => {
    const v = video.current;
    if (!v || verticalCenter === null) return;
    measureFrame();
    const observer = new ResizeObserver(measureFrame);
    observer.observe(v);
    return () => observer.disconnect();
  }, [measureFrame, verticalCenter === null]);
  // Retain the requested frame while Chromium asynchronously completes a seek.
  // Repeated key presses must advance, even before currentTime/seeked catches up.
  const requestedFrame = useRef<number | null>(null);
  const presentedFrame = useRef<number | null>(null);

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
  const { active: tracksActive } = useTrackSync(
    video,
    trackUrls,
    trackUrls.map((_, i) => trackVolume(trackStates[i] ?? { muted: false, gain: 100 }, level, muted)),
  );

  useEffect(() => {
    const v = video.current;
    if (!v) return;
    v.volume = level / 100;
    // The picture only, once the separate tracks carry the sound.
    v.muted = muted || tracksActive;
  }, [level, muted, src, tracksActive]);

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
    if (v.paused) { requestedFrame.current = null; void v.play(); }
    else v.pause();
  }, []);

  const seekTo = useCallback((ms: number) => {
    requestedFrame.current = null;
    presentedFrame.current = null;
    if (video.current) video.current.currentTime = ms / 1000;
  }, []);

  const seekFrame = useCallback((ms: number) => {
    const v = video.current;
    if (!v || !frameIndex?.timestamps_100ns.length || !Number.isFinite(ms)) return;
    const target = nearestFrameBoundary(ms, frameIndex);
    v.pause();
    requestedFrame.current = target;
    v.currentTime = target / 1000;
    setPositionMs(target);
    onTime(target);
  }, [frameIndex, onTime]);

  const currentFrame = useCallback(() => {
    if (requestedFrame.current !== null) return requestedFrame.current;
    // RVFC gives a frame's identity, potentially rounded to Chromium's media
    // clock precision. Match it to the nearest indexed PTS, not its predecessor.
    if (presentedFrame.current !== null) {
      return frameIndex ? nearestFrameBoundary(presentedFrame.current, frameIndex) : presentedFrame.current;
    }
    const ms = (video.current?.currentTime ?? 0) * 1000;
    // currentTime is a playback position between images, so its frame is floor.
    return frameIndex ? presentedFrameBoundary(ms, frameIndex) : ms;
  }, [frameIndex]);

  const stepFrame = useCallback((direction: 1 | -1) => {
    if (!video.current || !frameIndex?.timestamps_100ns.length) return;
    seekFrame(frameStep(currentFrame(), frameIndex, direction));
  }, [frameIndex, seekFrame, currentFrame]);

  useImperativeHandle(ref, () => ({ toggle, seekTo, seekFrame, stepFrame, currentFrame, stepVolume }), [toggle, seekTo, seekFrame, stepFrame, currentFrame, stepVolume]);

  useEffect(() => {
    const v = video.current;
    if (!v || typeof v.requestVideoFrameCallback !== 'function') return;
    let callbackId = 0;
    const presented: VideoFrameRequestCallback = (_, metadata) => {
      presentedFrame.current = metadata.mediaTime * 1000;
      callbackId = v.requestVideoFrameCallback(presented);
    };
    callbackId = v.requestVideoFrameCallback(presented);
    return () => v.cancelVideoFrameCallback(callbackId);
  }, [src]);

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
    requestedFrame.current = null;
    presentedFrame.current = null;
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
  const clock = frameIndex?.timestamps_100ns.length ? formatPreciseClock : formatClock;

  return (
    <div
      ref={stage}
      className={`pro-player${inspector ? ' pro-player--inspector' : ''}`}>
      <div className="pro-player-viewer">
      <video
        ref={video}
        onResize={measureFrame}
        src={src}
        autoPlay
        onPlay={() => { requestedFrame.current = null; setPlaying(true); }}
        onPause={() => setPlaying(false)}
        onSeeked={() => {
          const v = video.current;
          if (!v) return;
          const ms = v.paused && requestedFrame.current !== null ? requestedFrame.current : v.currentTime * 1000;
          setPositionMs(ms);
          onTime(ms);
        }}
        onLoadedMetadata={() => {
          measureFrame();
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
          const ms = v.paused && requestedFrame.current !== null ? requestedFrame.current : v.currentTime * 1000;
          setPositionMs(ms);
          onTime(ms);
        }}
        onClick={toggle}
        className="pro-player-video"
      />
      {verticalCenter !== null && frame && <VerticalCropOverlay frame={frame} center={verticalCenter} onChange={onVerticalCenter} />}
      </div>
      {inspector && <aside className="pro-player-inspector" aria-label="Réglages du montage">{inspector}</aside>}

      <div className="pro-player-transport">
        <span className="pro-player-clock">
          <strong>{clock(positionMs)}</strong><span>/</span>{clock(durationMs)}
        </span>
        <div className="pro-player-playback" role="group" aria-label="Lecture et navigation image par image">
          <Button color="secondary" variant="ghost" size="sm" uniform
            disabled={!frameIndex?.timestamps_100ns.length}
            aria-label="Image précédente" title="Image précédente (,)" onClick={() => stepFrame(-1)}>
            <ChevronLeft className="size-4" />
          </Button>
        <Button
          color="secondary"
          variant="ghost"
          size="sm"
          uniform
          aria-label={playing ? 'Pause' : 'Lecture'}
          className="shrink-0 transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]"
          onClick={toggle}>
          {playing ? <Pause className="size-4" /> : <Play className="size-4" />}
        </Button>

          <Button color="secondary" variant="ghost" size="sm" uniform
            disabled={!frameIndex?.timestamps_100ns.length}
            aria-label="Image suivante" title="Image suivante (.)" onClick={() => stepFrame(1)}>
            <ChevronRight className="size-4" />
          </Button>
        </div>

        <div className="pro-player-output">
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
      <div className="pro-player-timeline">{timeline(durationMs)}</div>
    </div>
  );
}

type PictureFrame = { left: number; top: number; width: number; height: number };

/** Dims what a 9:16 export leaves out; drag the band to move the crop. */
function VerticalCropOverlay({ frame, center, onChange }: { frame: PictureFrame; center: number; onChange?: (center: number) => void }) {
  const band = verticalBand(frame.width, frame.height, center);
  const drag = useRef<{ x: number; center: number } | null>(null);
  const travel = frame.width - band.width;
  const move = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || !onChange || travel <= 0) return;
    const delta = ((event.clientX - drag.current.x) / frame.width) * 1000;
    onChange(Math.round(Math.min(1000, Math.max(0, drag.current.center + delta))));
  };
  return (
    <div className="vertical-crop" style={{ left: frame.left, top: frame.top, width: frame.width, height: frame.height }} aria-hidden="true">
      <div className="vertical-crop-shade" style={{ left: 0, width: band.left }} />
      <div
        className="vertical-crop-band"
        style={{ left: band.left, width: band.width, cursor: onChange && travel > 0 ? 'ew-resize' : undefined }}
        onPointerDown={(event) => {
          if (!onChange) return;
          event.currentTarget.setPointerCapture(event.pointerId);
          // Start from the centre the band shows: a clamped one sits off its nominal value.
          drag.current = { x: event.clientX, center: travel > 0 ? ((band.left + band.width / 2) / frame.width) * 1000 : center };
        }}
        onPointerMove={move}
        onPointerUp={() => { drag.current = null; }}
        onPointerCancel={() => { drag.current = null; }}
      />
      <div className="vertical-crop-shade" style={{ left: band.left + band.width, right: 0 }} />
    </div>
  );
}
