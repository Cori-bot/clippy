import { describe, expect, it } from 'vitest';
import { ICONS, isSolid, type IconName } from './icons';

/**
 * Every name the app asks for by string somewhere. A missing entry is a
 * blank square in the UI and nothing in the console, so it is checked here
 * rather than discovered on screen.
 */
const REQUIRED: IconName[] = [
  'play', 'pause', 'clips', 'settings', 'star', 'star-filled', 'pencil',
  'trash', 'folder', 'scissors', 'volume', 'volume-low', 'volume-mute', 'fullscreen',
  'fullscreen-exit', 'chevron-left', 'chevron-right', 'chevron-down', 'dots',
  'check', 'rearm', 'minimize', 'maximize', 'restore', 'close', 'alert',
  'camera', 'copy',
];

describe('ICONS', () => {
  it('has every icon the app names', () => {
    for (const name of REQUIRED) {
      expect(ICONS[name], `missing icon: ${name}`).toBeDefined();
    }
  });

  it('gives every icon at least one non-empty path', () => {
    for (const [name, def] of Object.entries(ICONS)) {
      const ds = isSolid(def) ? def.paths.map((p) => p.d) : [def.d];
      expect(ds.length, `no path: ${name}`).toBeGreaterThan(0);
      for (const d of ds) expect(d.trim().length, `empty path: ${name}`).toBeGreaterThan(0);
    }
  });

  it('gives every solid icon a viewBox the component can use', () => {
    for (const [name, def] of Object.entries(ICONS)) {
      if (isSolid(def)) expect(def.viewBox, name).toMatch(/^0 0 \d+ \d+$/);
    }
  });

  it('keeps the window caption buttons as strokes', () => {
    for (const name of ['minimize', 'maximize', 'restore', 'close'] as const) {
      expect(isSolid(ICONS[name]), name).toBe(false);
    }
  });

  it('carries no icon the app does not ask for', () => {
    // YAGNI, enforced. An unused icon is dead weight in every bundle.
    expect(Object.keys(ICONS).sort()).toEqual([...REQUIRED].sort());
  });
});
