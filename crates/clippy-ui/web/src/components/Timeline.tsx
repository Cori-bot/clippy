import { useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { formatClock, formatPreciseClock, frameStep, keyframeStep, msFromRatio, nearestFrameBoundary, ratioFromMs, seekKeyTarget, snapStart, type FrameIndex } from '../lib/timeline';
import { clamp } from '../lib/ui';
import { Filmstrip, Waveform } from './TimelineMedia';

type Drag = 'in' | 'out' | 'seek' | null;

/**
 * The unified seek-and-trim band: one timeline for the playhead and for the
 * In/Out points.
 *
 * Presentational on purpose: it never calls the daemon. It is given a
 * duration, a playhead, a keyframe list and the current in/out, and reports
 * back where the user dragged.
 *
 * `trimmable` false leaves a seek-only band: the track, the playhead and both
 * ways of moving it stay, and everything that expresses a range (both handles,
 * the fill between them and the dimming outside them) is not rendered. For the
 * clip the daemon will not trim (`duration_ms: 0`). `onChange` cannot fire
 * while it is false: a trim drag in progress is dropped by `move`.
 */
export function Timeline({
  durationMs,
  playheadMs,
  keyframes,
  frameIndex,
  inMs,
  outMs,
  trimmable = true,
  src,
  trackUrls = [],
  trackNames = [],
  onChange,
  onSeek,
}: {
  durationMs: number;
  playheadMs: number;
  keyframes: number[];
  /** Provided only in precise mode; keyframes remain the sparse visual ticks. */
  frameIndex?: FrameIndex;
  inMs: number;
  outMs: number;
  trimmable?: boolean;
  src?: string;
  trackUrls?: string[];
  trackNames?: string[];
  onChange: (inMs: number, outMs: number) => void;
  onSeek: (ms: number) => void;
}) {
  const band = useRef<HTMLDivElement>(null);
  const [zoom, setZoom] = useState(1);
  /** Which handle a pointer is currently dragging, or `seek` for the track. */
  const [drag, setDrag] = useState<Drag>(null);
  // Mirrors `drag` for the handlers below, which can run twice before React
  // re-renders (pointerdown then pointermove in the same frame).
  const dragRef = useRef<Drag>(null);
  const setDragBoth = (d: Drag) => {
    dragRef.current = d;
    setDrag(d);
  };

  const pct = (ms: number) => `${ratioFromMs(ms, durationMs) * 100}%`;
  const clock = frameIndex?.timestamps_100ns.length ? formatPreciseClock : formatClock;

  const msAt = (clientX: number): number => {
    const el = band.current;
    if (!el) return 0;
    const box = el.getBoundingClientRect();
    return msFromRatio((clientX - box.left) / box.width, durationMs);
  };

  const move = (e: PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    // A trim drag cannot outlive `trimmable`: stepping to a clip the daemon
    // will not trim unmounts the handles mid-drag while `drag` still says `in`.
    if (!trimmable && d !== 'seek') {
      setDragBoth(null);
      return;
    }
    const ms = msAt(e.clientX);
    if (d === 'seek') onSeek(ms);
    // Snapped at the moment of the drag, so the handle sits where the export
    // will really cut, not where the pointer was.
    else if (d === 'in') onChange(frameIndex ? nearestFrameBoundary(ms, frameIndex) : snapStart(ms, keyframes), outMs);
    // Out is reported raw: fast mode cuts the end where it is asked to.
    else onChange(inMs, frameIndex ? nearestFrameBoundary(ms, frameIndex, true) : ms);
  };

  const grab = (which: 'in' | 'out' | 'seek', e: PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragBoth(which);
    move(e);
  };

  const drop = (e: PointerEvent) => {
    if (!dragRef.current) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    setDragBoth(null);
  };

  /**
   * Arrow keys on a focused handle. In moves by whole keyframes, the only unit
   * an in-point has. Out moves by a tenth of a second, or a whole one with Shift.
   */
  const onHandleKey = (which: 'in' | 'out', e: KeyboardEvent) => {
    const dir = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    let next: number;
    if (dir !== 0) {
      if (frameIndex?.timestamps_100ns.length) {
        next = frameStep(which === 'in' ? inMs : outMs, frameIndex, dir, which === 'out');
      } else if (which === 'in') next = keyframeStep(inMs, keyframes, dir);
      else next = clamp(outMs + dir * (e.shiftKey ? 1000 : 100), 0, durationMs);
    } else if (e.key === 'Home') next = frameIndex?.timestamps_100ns[0] !== undefined ? frameIndex.timestamps_100ns[0] / 10_000 : 0;
    else if (e.key === 'End') next = frameIndex?.timestamps_100ns.length
      ? (which === 'out' ? frameIndex.end_100ns : frameIndex.timestamps_100ns.at(-1)!) / 10_000
      : durationMs;
    else return;
    // Stopped as well as prevented: `ClipPage` listens on the window and would
    // otherwise also step to the next clip.
    e.preventDefault();
    e.stopPropagation();
    if (which === 'in') onChange(clamp(next, 0, durationMs), outMs);
    else onChange(inMs, clamp(next, 0, durationMs));
  };

  /** Arrow keys on the focused playhead; keys it does not own are left alone. */
  const onSeekKey = (e: KeyboardEvent) => {
    const next = seekKeyTarget(playheadMs, durationMs, e.key, e.shiftKey);
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    onSeek(next);
  };

  const handle = (which: 'in' | 'out', label: string, ms: number) => (
    // A generous hit area surrounds the narrow trim edge without obscuring frames.
    <span
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={durationMs}
      aria-valuenow={ms}
      aria-valuetext={clock(ms)}
      className={`timeline-trim timeline-trim--${which}${drag === which ? ' is-dragging' : ''}`}
      style={{ left: pct(ms) }}
      onPointerDown={(e) => grab(which, e)}
      onPointerMove={move}
      onPointerUp={drop}
      onPointerCancel={() => setDragBoth(null)}
      onLostPointerCapture={() => setDragBoth(null)}
      onKeyDown={(e) => onHandleKey(which, e)}>
      <span className="timeline-trim-grip" />
    </span>
  );

  // The band itself is a pointer convenience: press or drag anywhere along it
  // to seek. The playhead inside it is a real focusable slider, so seeking is
  // not pointer-only: a keyboard moves it in 5s steps, 1s with Shift, and
  // jumps to either end. The band cannot take the slider role itself: `slider`
  // is a leaf role and this element already contains one.
  const targetTick = durationMs / (8 * zoom);
  const tick = [100, 200, 500, 1000, 2000, 5000, 10000, 15000, 30000, 60000, 120000, 300000, 600000].find((value) => value >= targetTick) ?? Math.max(600000, targetTick);
  const tickCount = Math.min(200, Math.floor(durationMs / tick));
  return (
    <section className="editor-timeline" aria-label="Timeline du clip">
      <div className="timeline-toolbar">
        <h2>Timeline</h2>
        <span className="timeline-shortcut">Glisser les bords pour découper</span>
        <label className="timeline-zoom">Zoom
          <input type="range" min="1" max="8" step="0.5" value={zoom}
            aria-label="Zoom de la timeline" aria-valuetext={`${zoom} fois`}
            onChange={(e) => setZoom(Number(e.target.value))}
            onKeyDown={(e) => e.stopPropagation()} />
          <output>{zoom}×</output>
        </label>
      </div>
      <div className="timeline-lanes">
        <div className="timeline-labels" aria-hidden="true">
          <div className="timeline-ruler-label" />
          <div className="timeline-video-label">Vidéo</div>
          {trackUrls.map((url, i) => <div key={`${url}-${i}`} className="timeline-audio-label" title={trackNames[i]}>{trackNames[i] ?? `Audio ${i + 1}`}</div>)}
        </div>
        <div className="timeline-scroll">
    <div
      ref={band}
      className="timeline-band"
      style={{ width: `${zoom * 100}%` }}
      onPointerDown={(e) => grab('seek', e)}
      onPointerMove={move}
      onPointerUp={drop}
      onPointerCancel={() => setDragBoth(null)}
      onLostPointerCapture={() => setDragBoth(null)}>
      <div className="timeline-ruler" aria-hidden="true">
        {Array.from({ length: tickCount + 1 }, (_, i) => <span key={i} style={{ left: pct(i * tick) }}>{formatClock(i * tick)}</span>)}
      </div>
      <Filmstrip src={src} />
      {trackUrls.map((url, i) => <Waveform key={`${url}-${i}`} src={url} durationMs={durationMs} />)}
      {keyframes.filter((k) => k >= 0 && k <= durationMs).map((k) => (
        <span
          key={k}
          className="timeline-keyframe"
          style={{ left: pct(k) }}
        />
      ))}

      {trimmable && (
        <>
          <span className="timeline-outside timeline-outside--before" style={{ width: pct(inMs) }} />
          <span className="timeline-outside timeline-outside--after" style={{ left: pct(outMs) }} />
          <span
            className="timeline-selection"
            style={{
              left: pct(inMs),
              width: `${ratioFromMs(Math.max(0, outMs - inMs), durationMs) * 100}%`,
            }}
          />
        </>
      )}

      <span
        role="slider"
        tabIndex={0}
        aria-label="Position de lecture"
        aria-valuemin={0}
        aria-valuemax={durationMs}
        aria-valuenow={playheadMs}
        aria-valuetext={clock(playheadMs)}
        className="timeline-playhead"
        style={{ left: pct(playheadMs) }}
        onKeyDown={onSeekKey}
      />

      {trimmable && (
        <>
          {handle('in', 'Point d’entrée', inMs)}
          {handle('out', 'Point de sortie', outMs)}
        </>
      )}
    </div>
        </div>
      </div>
      <div className="timeline-footer"><span><kbd>,</kbd> / <kbd>.</kbd> Image par image</span><span><kbd>I</kbd> Entrée <kbd>O</kbd> Sortie</span></div>
    </section>
  );
}
