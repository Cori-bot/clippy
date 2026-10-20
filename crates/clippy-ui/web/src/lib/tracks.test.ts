import { describe, expect, it } from 'vitest';
import {
  buildExportTracks,
  clampGain,
  defaultStates,
  editableTracks,
  needsResync,
  trackVolume,
  tracksChanged,
} from './tracks';

describe('editableTracks', () => {
  it('skips the Mix and keeps file stream indexes', () => {
    expect(editableTracks(['Mix', 'Jeu', 'Micro'])).toEqual([
      { index: 1, name: 'Jeu' },
      { index: 2, name: 'Micro' },
    ]);
  });
  it('offers nothing for a single stream, none, or an old clip', () => {
    expect(editableTracks(['Audio'])).toEqual([]);
    expect(editableTracks([])).toEqual([]);
    expect(editableTracks(undefined)).toEqual([]);
  });
});

describe('buildExportTracks', () => {
  const tracks = editableTracks(['Mix', 'Jeu', 'Micro']);
  it('is undefined while nothing changed', () => {
    expect(tracksChanged(defaultStates(2))).toBe(false);
    expect(buildExportTracks(tracks, defaultStates(2))).toBeUndefined();
  });
  it('leaves muted tracks out and keeps gains', () => {
    const s = defaultStates(2);
    s[0] = { muted: true, gain: 100 };
    s[1] = { muted: false, gain: 150 };
    expect(buildExportTracks(tracks, s)).toEqual([{ index: 2, gain: 150 }]);
  });
  it('sends every track when only a gain changed', () => {
    const s = defaultStates(2);
    s[0] = { muted: false, gain: 50 };
    expect(buildExportTracks(tracks, s)).toEqual([
      { index: 1, gain: 50 },
      { index: 2, gain: 100 },
    ]);
  });
  it('sends an empty list when everything is muted', () => {
    const s = [{ muted: true, gain: 100 }, { muted: true, gain: 100 }];
    expect(buildExportTracks(tracks, s)).toEqual([]);
  });
});

describe('clampGain', () => {
  it('stays within 0..200', () => {
    expect(clampGain(-5)).toBe(0);
    expect(clampGain(250)).toBe(200);
    expect(clampGain(NaN)).toBe(100);
  });
});

describe('needsResync', () => {
  it('tolerates 80 ms and no more', () => {
    expect(needsResync(10.05, 10)).toBe(false);
    expect(needsResync(10.2, 10)).toBe(true);
    expect(needsResync(9.8, 10)).toBe(true);
  });
});

describe('trackVolume', () => {
  it('multiplies track level by the master', () => {
    expect(trackVolume({ muted: false, gain: 50 }, 50, false)).toBeCloseTo(0.25);
  });
  it('is silent when muted at either level', () => {
    expect(trackVolume({ muted: true, gain: 100 }, 100, false)).toBe(0);
    expect(trackVolume({ muted: false, gain: 100 }, 100, true)).toBe(0);
  });
  it('caps boosted tracks at full volume in the preview', () => {
    expect(trackVolume({ muted: false, gain: 200 }, 100, false)).toBe(1);
  });
});
