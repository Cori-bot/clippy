import { describe, expect, it } from 'vitest';
import { SEEK_STEP_FINE_MS, SEEK_STEP_MS, formatClock, formatPreciseClock, frameStep, keyframeStep, msFromRatio, nearestFrameBoundary, presentedFrameBoundary, ratioFromMs, seekKeyTarget, snapStart } from './timeline';

describe('real frame boundaries', () => {
  const variable = { timestamps_100ns: [0, 166667, 500000, 916667], end_100ns: 1_200_000 };

  it('keeps adjacent frames distinct in the displayed time', () => {
    expect(formatPreciseClock(16.6667)).toBe('0:00,017');
    expect(formatPreciseClock(33.3333)).toBe('0:00,033');
    expect(formatPreciseClock(59999.9)).toBe('1:00,000');
  });

  it('finds the actual 60fps frame after rounding its input to 17ms', () => {
    expect(nearestFrameBoundary(17, variable)).toBe(16.6667);
  });

  it('matches a quantized RVFC frame identity without confusing it with playback position', () => {
    expect(nearestFrameBoundary(16.666, variable)).toBe(16.6667);
    expect(frameStep(nearestFrameBoundary(16.666, variable), variable, 1)).toBe(50);
    expect(presentedFrameBoundary(16.666, variable)).toBe(0);
  });

  it('uses earlier boundaries on exact ties, including the exclusive end', () => {
    const regular = { timestamps_100ns: [0, 1_000_000, 2_000_000], end_100ns: 3_000_000 };
    expect(nearestFrameBoundary(150, regular)).toBe(100);
    expect(nearestFrameBoundary(250, regular, true)).toBe(200);
    expect(nearestFrameBoundary(290, regular, true)).toBe(300);
    expect(nearestFrameBoundary(300, regular)).toBe(200);
  });

  it('steps across variable intervals rather than inventing a nominal frame rate', () => {
    expect(frameStep(16.6667, variable, 1)).toBe(50);
    expect(frameStep(50, variable, 1)).toBe(91.6667);
    expect(frameStep(50, variable, -1)).toBe(16.6667);
    // At 70ms the picture on screen is the frame presented at 50ms.
    expect(frameStep(70, variable, -1)).toBe(16.6667);
    expect(frameStep(70, variable, 1)).toBe(91.6667);
  });

  it('marks the currently displayed image instead of rounding to a future frame', () => {
    expect(presentedFrameBoundary(45, variable)).toBe(16.6667);
    expect(presentedFrameBoundary(50, variable)).toBe(50);
    expect(presentedFrameBoundary(120, variable)).toBe(91.6667);
  });

  it('steps back from the exclusive out boundary but never treats it as a display frame', () => {
    expect(frameStep(120, variable, -1, true)).toBe(91.6667);
    expect(frameStep(91.6667, variable, 1, true)).toBe(120);
    expect(frameStep(91.6667, variable, 1)).toBe(91.6667);
    expect(frameStep(0, variable, -1)).toBe(0);
  });

  it('does not fabricate frames before a nonzero first timestamp or with an empty index', () => {
    expect(nearestFrameBoundary(0, { timestamps_100ns: [500000, 900000], end_100ns: 1_000_000 })).toBe(50);
    expect(frameStep(50, { timestamps_100ns: [], end_100ns: 1000000 }, 1)).toBe(50);
  });
});

describe('snapStart', () => {
  it('moves back to the latest keyframe at or before the point', () => {
    expect(snapStart(2500, [0, 1000, 2000, 3000])).toBe(2000);
    expect(snapStart(2000, [0, 1000, 2000, 3000])).toBe(2000);
  });

  it('returns the point untouched when there are no keyframes', () => {
    // Not "snap to zero". An empty list means the ticks are still loading, or
    // the clip could not be indexed at all -- and pinning the in-point to 0
    // in either case makes the control look broken. The daemon snaps it for
    // real regardless.
    expect(snapStart(2500, [])).toBe(2500);
  });

  it('returns 0 when every keyframe is later than the point', () => {
    expect(snapStart(500, [1000, 2000])).toBe(0);
  });
});

describe('keyframeStep', () => {
  it('moves to the neighbouring keyframe', () => {
    expect(keyframeStep(2000, [0, 1000, 2000, 3000], 1)).toBe(3000);
    expect(keyframeStep(2000, [0, 1000, 2000, 3000], -1)).toBe(1000);
  });

  it('moves off an off-grid point to the neighbour in that direction', () => {
    expect(keyframeStep(2400, [0, 1000, 2000, 3000], 1)).toBe(3000);
    expect(keyframeStep(2400, [0, 1000, 2000, 3000], -1)).toBe(2000);
  });

  it('stays put at either end of the list', () => {
    expect(keyframeStep(3000, [0, 1000, 2000, 3000], 1)).toBe(3000);
    expect(keyframeStep(0, [0, 1000, 2000, 3000], -1)).toBe(0);
  });

  it('falls back to one second when there are no keyframes', () => {
    // Same honesty as snapStart: with nothing to step between, a whole
    // second is a defensible unit and pretending otherwise is not.
    expect(keyframeStep(4000, [], 1)).toBe(5000);
    expect(keyframeStep(4000, [], -1)).toBe(3000);
    expect(keyframeStep(300, [], -1)).toBe(0);
  });
});

describe('msFromRatio / ratioFromMs', () => {
  it('maps both ends and clamps a pointer dragged past them', () => {
    expect(msFromRatio(0, 15000)).toBe(0);
    expect(msFromRatio(1, 15000)).toBe(15000);
    expect(msFromRatio(-0.5, 15000)).toBe(0);
    expect(msFromRatio(2, 15000)).toBe(15000);
  });

  it('rounds to a whole millisecond', () => {
    expect(msFromRatio(1 / 3, 10000)).toBe(3333);
  });

  it('gives 0 for a clip with no duration rather than NaN', () => {
    // `library::scan` adopts a bare .mp4 as `duration_ms: 0`, and `NaN%`
    // in CSS drops the playhead out of the band entirely.
    expect(ratioFromMs(500, 0)).toBe(0);
    expect(msFromRatio(0.5, 0)).toBe(0);
  });
});

describe('formatClock', () => {
  it('shows minutes, seconds and a tenth', () => {
    expect(formatClock(0)).toBe('0:00,0');
    expect(formatClock(5700)).toBe('0:05,7');
    expect(formatClock(65400)).toBe('1:05,4');
  });

  it('never shows a negative time', () => {
    expect(formatClock(-20)).toBe('0:00,0');
  });
});

describe('seekKeyTarget', () => {
  it('moves the playhead by a whole step on the arrows', () => {
    expect(seekKeyTarget(10_000, 60_000, 'ArrowRight', false)).toBe(10_000 + SEEK_STEP_MS);
    expect(seekKeyTarget(10_000, 60_000, 'ArrowLeft', false)).toBe(10_000 - SEEK_STEP_MS);
  });

  it('takes a finer grain with Shift', () => {
    expect(seekKeyTarget(10_000, 60_000, 'ArrowRight', true)).toBe(10_000 + SEEK_STEP_FINE_MS);
    expect(seekKeyTarget(10_000, 60_000, 'ArrowLeft', true)).toBe(10_000 - SEEK_STEP_FINE_MS);
  });

  it('jumps to either end', () => {
    expect(seekKeyTarget(10_000, 60_000, 'Home', false)).toBe(0);
    expect(seekKeyTarget(10_000, 60_000, 'End', false)).toBe(60_000);
  });

  it('stops at the ends rather than running past them', () => {
    expect(seekKeyTarget(1_000, 60_000, 'ArrowLeft', false)).toBe(0);
    expect(seekKeyTarget(59_000, 60_000, 'ArrowRight', false)).toBe(60_000);
  });

  it('returns null for a key the band does not own, so the page still gets it', () => {
    // ClipPage steps between clips on these. If this returned a number the
    // band would swallow them and the page would lose its prev/next.
    expect(seekKeyTarget(10_000, 60_000, 'ArrowUp', false)).toBeNull();
    expect(seekKeyTarget(10_000, 60_000, ' ', false)).toBeNull();
    expect(seekKeyTarget(10_000, 60_000, 'Escape', false)).toBeNull();
  });

  it('cannot move a clip with no duration off zero', () => {
    expect(seekKeyTarget(0, 0, 'ArrowRight', false)).toBe(0);
    expect(seekKeyTarget(0, 0, 'End', false)).toBe(0);
  });
});

describe('ratioFromMs past the end', () => {
  it('does not report more than the whole clip for a playhead past the end', () => {
    // The mirror of msFromRatio's ratio > 1 case. A stale playhead arriving
    // after a shorter clip loads would otherwise position the marker outside
    // the band.
    expect(ratioFromMs(90_000, 60_000)).toBe(1);
  });
});
