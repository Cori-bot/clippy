import { describe, expect, it } from 'vitest';
import { capError, longestForCap, SIZE_PRESETS, verticalBand } from './share';

describe('share size caps', () => {
  it('offers the Discord upload limits in increasing order', () => {
    expect(SIZE_PRESETS.map((p) => p.mb)).toEqual([20, 50, 500]);
  });

  it('matches the daemon floor: 90 % of the cap over audio plus 150 kb/s video', () => {
    // 20 MB * 8 * 0.9 = 144 Mb over 342 kb/s
    expect(longestForCap(20, true)).toBe(Math.floor((144_000_000 / 342_000) * 1000));
    expect(longestForCap(20, false)).toBe(960_000);
  });

  it('accepts a selection that fits and explains one that does not', () => {
    expect(capError(null, 10_000_000, true)).toBeNull();
    expect(capError(20, 60_000, true)).toBeNull();
    const error = capError(20, 600_000, true);
    expect(error).toContain('20 Mo');
    expect(error).toContain('421 s');
  });
});

describe('vertical band', () => {
  it('is 9:16 of the height, centred and clamped inside the picture', () => {
    expect(verticalBand(1920, 1080, 500)).toEqual({ left: 656.25, width: 607.5 });
    expect(verticalBand(1920, 1080, 0).left).toBe(0);
    expect(verticalBand(1920, 1080, 1000).left).toBe(1920 - 607.5);
    expect(verticalBand(1080, 1920, 500)).toEqual({ left: 0, width: 1080 });
  });
});
