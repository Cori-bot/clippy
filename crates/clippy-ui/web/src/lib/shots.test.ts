import { describe, expect, it, vi } from 'vitest';

// `convertFileSrc` needs a Tauri host, so it is stubbed to return its input.
// What is under test is the path handed to it, which is the half that has
// actually been wrong before.
vi.mock('@tauri-apps/api/core', () => ({ convertFileSrc: (p: string) => p }));

const { shotUrl, shotThumbUrl, formatShotDate } = await import('./shots');

describe('formatShotDate', () => {
  it('writes the date and time in French', () => {
    expect(formatShotDate('2026-08-27T14:30:12Z', 'UTC')).toBe('27 août 2026 à 14:30:12');
  });
});

describe('screenshot URLs', () => {
  it('points into the Screenshots subfolder, not the clips folder', () => {
    // A screenshot beside the clips would collide with a clip's own {id}.jpg
    // thumbnail. The subfolder prevents that, and it has to be in the URL or
    // the tab renders clip thumbnails instead.
    expect(shotUrl('D:\\Videos\\Clippy', '20260827_143012')).toBe(
      'D:\\Videos\\Clippy\\Screenshots\\20260827_143012.jpg',
    );
    // The grid thumbnail is in the library cache, so the folder keeps images only.
    expect(shotThumbUrl('C:\\cache\\', '20260827_143012')).toBe('C:\\cache\\shots\\20260827_143012.jpg');
  });

  it('strips a trailing separator, including on a drive root', () => {
    // `clip_dir` is a config key a person types. A drive root carries a
    // trailing separator by necessity, and the doubled separator that results
    // can miss the asset-scope glob -- a broken image with nothing logged.
    expect(shotUrl('D:\\', '20260827_143012')).toBe('D:\\Screenshots\\20260827_143012.jpg');
    expect(shotUrl('D:\\Videos\\Clippy\\', '20260827_143012')).toBe(
      'D:\\Videos\\Clippy\\Screenshots\\20260827_143012.jpg',
    );
  });
});
