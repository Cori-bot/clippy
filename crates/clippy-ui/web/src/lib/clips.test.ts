import { describe, expect, it, vi } from 'vitest';
import type { ClipMeta } from './types';

// The real `convertFileSrc` reads `window.__TAURI_INTERNALS__`, which does not
// exist outside the webview. Returning the path it was handed is what makes the
// assertions below about the *path* rather than about Tauri's URL scheme, which
// is Tauri's to change.
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string) => path,
}));

const { clipUrl, thumbUrl, trackUrl, formatBytes, formatDuration, mergeSaved, counterLabel } =
  await import('./clips');

const clip = (id: string, favorite = false): ClipMeta => ({
  id,
  title: `clip_${id}`,
  created: '2026-07-26T14:30:12+03:00',
  duration_ms: 20016,
  bytes: 19812352,
  width: 1920,
  height: 1200,
  fps: 60,
  encoder: 'NVENC H.264',
  has_audio: true,
  favorite,
});

describe('clipUrl and thumbUrl', () => {
  const id = '20260726_143012';
  const ref = { id };

  it('put the sidecar beside its clip, flat', () => {
    expect(clipUrl('C:\\Users\\me\\Videos\\Clippy', ref)).toBe(
      `C:\\Users\\me\\Videos\\Clippy\\${id}.mp4`,
    );
    expect(thumbUrl('C:\\Users\\me\\Videos\\Clippy', ref)).toBe(
      `C:\\Users\\me\\Videos\\Clippy\\${id}.jpg`,
    );
  });

  it('does not double the separator on a clip_dir that ends in one', () => {
    // `clip_dir` is typed into settings, and a doubled separator can miss the
    // asset-scope glob -- which shows up as every thumbnail in the grid being
    // broken, with no error anywhere.
    expect(thumbUrl('D:\\clips\\', ref)).toBe(`D:\\clips\\${id}.jpg`);
    expect(thumbUrl('D:/clips/', ref)).toBe(`D:/clips\\${id}.jpg`);
    expect(clipUrl('D:\\clips\\', ref)).toBe(`D:\\clips\\${id}.mp4`);
  });

  it('looks inside the clip folder when there is one', () => {
    expect(clipUrl('D:\\clips\\', { id, folder: 'Valorant' })).toBe(`D:\\clips\\Valorant\\${id}.mp4`);
    expect(thumbUrl('D:\\clips', { id, folder: '' })).toBe(`D:\\clips\\${id}.jpg`);
    expect(trackUrl('D:\\clips', { id, folder: 'Valorant' }, `${id}.t1.m4a`)).toBe(
      `D:\\clips\\Valorant\\${id}.t1.m4a`,
    );
  });

  it('resolves private editor audio within root and game folders', () => {
    const file = `.clippy-cache/${id}.t1.m4a`;
    expect(trackUrl('D:\\clips', ref, file)).toBe(`D:\\clips\\${file}`);
    expect(trackUrl('D:\\clips', { id, folder: 'Valorant' }, file)).toBe(
      `D:\\clips\\Valorant\\${file}`,
    );
  });

  it('handles a drive root, whose trailing separator is not optional', () => {
    expect(thumbUrl('D:\\', ref)).toBe(`D:\\${id}.jpg`);
    expect(clipUrl('D:\\', ref)).toBe(`D:\\${id}.mp4`);
  });
});

describe('formatDuration', () => {
  it('reads as a clip length, not as milliseconds', () => {
    expect(formatDuration(20016)).toBe('0:20');
    expect(formatDuration(65000)).toBe('1:05');
    expect(formatDuration(0)).toBe('0:00');
  });
});

describe('formatBytes', () => {
  it('uses the unit a person would', () => {
    expect(formatBytes(19812352)).toBe('18,9 Mo');
    expect(formatBytes(1024)).toBe('1,0 Ko');
    expect(formatBytes(0)).toBe('0 o');
    expect(formatBytes(5 * 1024 ** 3)).toBe('5,0 Go');
  });
});

describe('mergeSaved', () => {
  it('puts a new clip at the front, because the grid is newest-first', () => {
    const list = [clip('20260726_120000')];
    expect(mergeSaved(list, clip('20260726_143012'))[0].id).toBe('20260726_143012');
  });

  it('replaces rather than duplicates when the id is already there', () => {
    // library.list and a clip_saved event can race after a reconnect; two
    // cards for one file is the visible bug that causes.
    const list = [clip('20260726_143012'), clip('20260726_120000')];
    const merged = mergeSaved(list, clip('20260726_143012', true));
    expect(merged).toHaveLength(2);
    expect(merged[0].favorite).toBe(true);
  });
});

describe('counterLabel', () => {
  it('is one-based, the way spec §6.2 writes it', () => {
    expect(counterLabel(0, 47)).toBe('1 sur 47');
    expect(counterLabel(2, 47)).toBe('3 sur 47');
  });

  it('says nothing when there is nothing', () => {
    expect(counterLabel(0, 0)).toBe('');
  });
});
