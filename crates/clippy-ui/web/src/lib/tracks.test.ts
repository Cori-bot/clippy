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
  it('previews the full source gain used by export', () => {
    expect(trackVolume({ muted: false, gain: 200 }, 100, false)).toBe(2);
    expect(trackVolume({ muted: false, gain: 150 }, 50, false)).toBe(0.75);
    const tracks = editableTracks(['Mix', 'Micro']);
    const states = [{ muted: false, gain: 175 }];
    expect(trackVolume(states[0], 100, false)).toBe(buildExportTracks(tracks, states)![0].gain / 100);
  });
  it('bounds invalid master and source levels without producing an invalid audio gain', () => {
    expect(trackVolume({ muted: false, gain: 250 }, 120, false)).toBe(2);
    expect(trackVolume({ muted: false, gain: 100 }, -10, false)).toBe(0);
    expect(trackVolume({ muted: false, gain: 100 }, NaN, false)).toBe(0);
  });
});
