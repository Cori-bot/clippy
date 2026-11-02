import { convertFileSrc } from '@tauri-apps/api/core';
import type { ClipMeta } from './types';

/** What locates a clip's files: its id, its file name (the title) and the
 * sub-folder it lives in. */
export type ClipRef = Pick<ClipMeta, 'id' | 'folder' | 'title'>;

/**
 * Strips the trailing separator a typed path (or a drive root, `D:\`) carries:
 * a doubled one can miss the asset-scope glob Tauri builds from the same
 * directory, and then every image is broken with nothing logged.
 */
function trimDir(dir: string): string {
  return dir.replace(/[\\/]+$/, '');
}

/**
 * The clip's own file: its title is its name, in its folder ("VALORANT
 * 2026-11-02 14-27-22.mp4"). The folder holds the videos and nothing else.
 */
function clipFile(clipDir: string, clip: ClipRef): string {
  const base = trimDir(clipDir);
  const folder = (clip.folder ?? '').replace(/^[\\/]+|[\\/]+$/g, '');
  const name = `${clip.title}.mp4`;
  return folder ? `${base}\\${folder}\\${name}` : `${base}\\${name}`;
}

/**
 * Playable URL for a clip.
 *
 * `asset:` rather than reading bytes through IPC: the asset protocol answers
 * Range requests, which is what makes seeking in a 20 s 1080p60 MP4 work at
 * all. Sending the file over IPC would mean buffering the whole clip in the
 * webview before the first frame.
 */
export function clipUrl(clipDir: string, clip: ClipRef): string {
  return convertFileSrc(clipFile(clipDir, clip));
}

/** The grid thumbnail, kept in the library cache (`status.cache_dir`). */
export function thumbUrl(cacheDir: string, clip: Pick<ClipMeta, 'id'>): string {
  return convertFileSrc(`${trimDir(cacheDir)}\\thumbs\\${clip.id}.jpg`);
}

/** A per-track audio file the daemon extracted; `library.tracks` gives its
 * absolute path in the library cache. */
export function trackUrl(file: string): string {
  return convertFileSrc(file);
}

export function formatDuration(ms: number): string {
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

export function formatBytes(bytes: number): string {
  // French units (octets) and a decimal comma, with a no-break space so the
  // number never wraps away from its unit.
  if (bytes < 1024) return `${bytes} o`;
  const units = ['Ko', 'Mo', 'Go'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(1).replace('.', ',')} ${units[unit]}`;
}

/**
 * Folds a `clip_saved` (or a renamed/favorited clip) into the list.
 *
 * Replaces by id rather than always prepending: a reconnect re-runs
 * `library.list` while events are still arriving, and two cards for one file
 * is what that race looks like on screen.
 */
export function mergeSaved(clips: ClipMeta[], saved: ClipMeta): ClipMeta[] {
  const existing = clips.findIndex((c) => c.id === saved.id);
  if (existing >= 0) {
    const copy = clips.slice();
    copy[existing] = saved;
    return copy;
  }
  return [saved, ...clips];
}

export function counterLabel(index: number, total: number): string {
  return total === 0 ? '' : `${index + 1} sur ${total}`;
}
