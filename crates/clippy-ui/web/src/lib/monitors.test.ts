import { describe, expect, it } from 'vitest';
import { isPortrait, monitorLabel, monitorLayout } from './monitors';
import type { Monitor } from './types';

const m = (index: number, left: number, top: number, width: number, height: number): Monitor =>
  ({ index, name: `\\\\.\\DISPLAY${index + 1}`, left, top, width, height, adapter: 'GPU' });

describe('monitors', () => {
  it('names a screen by number, size, orientation and role, not by its device path', () => {
    expect(monitorLabel(m(0, 0, 0, 1920, 1080))).toBe('Écran 1 · 1920×1080 · horizontal · principal');
    expect(monitorLabel(m(1, 1920, -420, 1080, 1920))).toBe('Écran 2 · 1080×1920 · vertical');
    expect(isPortrait(m(1, 0, 0, 1080, 1920))).toBe(true);
  });

  it('keeps the arrangement, proportions and the box inside the bounds', () => {
    const screens = [m(0, 0, 0, 1920, 1080), m(1, 1920, -420, 1080, 1920), m(2, -1920, 0, 1920, 1080)];
    const layout = monitorLayout(screens, 260, 110);
    expect(layout.width).toBeLessThanOrEqual(260);
    expect(layout.height).toBeLessThanOrEqual(110);
    const [main, upright, left] = layout.boxes;
    expect(left.x).toBeLessThan(main.x);
    expect(upright.x).toBeGreaterThan(main.x);
    expect(upright.h).toBeGreaterThan(upright.w);
    expect(upright.y).toBeLessThan(main.y);
    expect(main.w).toBeGreaterThan(main.h);
  });

  it('lays out nothing without screens', () => {
    expect(monitorLayout([], 260, 110).boxes).toEqual([]);
  });
});
