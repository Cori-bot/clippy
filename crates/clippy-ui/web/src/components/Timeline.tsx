import { useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { formatClock, keyframeStep, msFromRatio, ratioFromMs, seekKeyTarget, snapStart } from '../lib/timeline';
import { clamp } from '../lib/ui';

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
  inMs,
  outMs,
  trimmable = true,
  onChange,
  onSeek,
}: {
  durationMs: number;
  playheadMs: number;
  keyframes: number[];
  inMs: number;
  outMs: number;
  trimmable?: boolean;
  onChange: (inMs: number, outMs: number) => void;
  onSeek: (ms: number) => void;
}) {
  const band = useRef<HTMLDivElement>(null);
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
    else if (d === 'in') onChange(snapStart(ms, keyframes), outMs);
    // Out is reported raw: fast mode cuts the end where it is asked to.
    else onChange(inMs, ms);
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
      if (which === 'in') next = keyframeStep(inMs, keyframes, dir);
      else next = clamp(outMs + dir * (e.shiftKey ? 1000 : 100), 0, durationMs);
    } else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = durationMs;
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
    // 32px hit area around a 24px visible knob; the knob is the focus target's
    // visual, the wrapper is the slider so both clicks and focus land on it.
    <span
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={durationMs}
      aria-valuenow={ms}
      aria-valuetext={formatClock(ms)}
      className="group/h absolute top-1/2 grid h-8 w-8 -translate-x-1/2 -translate-y-1/2 cursor-ew-resize place-items-center rounded-full outline-none"
      style={{ left: pct(ms) }}
      onPointerDown={(e) => grab(which, e)}
      onPointerMove={move}
      onPointerUp={drop}
      onKeyDown={(e) => onHandleKey(which, e)}>
      <span
        className={`size-6 rounded-full border-2 border-[var(--color-text)] bg-[var(--color-background-primary-solid)] shadow-sm transition-[transform,box-shadow] duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] group-hover/h:scale-110 group-focus-visible/h:ring-2 group-focus-visible/h:ring-[var(--color-border-strong)] ${
          drag === which ? 'scale-110 ring-4 ring-[var(--color-background-primary-soft-alpha)]' : ''
        }`}
      />
    </span>
  );

  // The band itself is a pointer convenience: press or drag anywhere along it
  // to seek. The playhead inside it is a real focusable slider, so seeking is
  // not pointer-only: a keyboard moves it in 5s steps, 1s with Shift, and
  // jumps to either end. The band cannot take the slider role itself: `slider`
  // is a leaf role and this element already contains one.
  return (
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions
    <div
      ref={band}
      className="relative h-10 cursor-pointer touch-none rounded-lg border border-subtle bg-surface-secondary"
      onPointerDown={(e) => grab('seek', e)}
      onPointerMove={move}
      onPointerUp={drop}>
      {keyframes.map((k) => (
        <span
          key={k}
          className="absolute inset-y-0 w-px bg-[var(--color-border-strong)]"
          style={{ left: pct(k) }}
        />
      ))}

      {trimmable && (
        <>
          <span className="pointer-events-none absolute inset-y-0 left-0 bg-black/40" style={{ width: pct(inMs) }} />
          <span className="pointer-events-none absolute inset-y-0 right-0 bg-black/40" style={{ left: pct(outMs) }} />
          <span
            className="pointer-events-none absolute inset-y-0 border-x-2 border-[var(--color-background-primary-solid)] bg-[var(--color-background-primary-soft-alpha)]"
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
        aria-valuetext={formatClock(playheadMs)}
        className="pointer-events-none absolute inset-y-0 w-0.5 bg-[var(--color-text)] outline-none before:absolute before:-top-px before:-left-[3px] before:h-[5px] before:w-2 before:rounded-b-sm before:bg-[var(--color-text)] focus-visible:shadow-[0_0_0_3px_var(--color-border-strong)]"
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
  );
}
