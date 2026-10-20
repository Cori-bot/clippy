import { convertFileSrc } from '@tauri-apps/api/core';
import type { ClipMeta } from './types';

/**
 * Every sidecar is the id plus a suffix, in the clip's folder.
 *
 * The trailing separator is stripped first because `clip_dir` is a config key
 * a person types. A drive root (`D:\`) has one by necessity and a hand-typed
 * path often picks one up, and either produced `D:\\20260726_143012.jpg` —
 * which can miss the asset-scope glob Tauri builds from the same directory, and
 * then every thumbnail in the grid is a broken image with nothing logged and
 * nothing on screen to say why. `D:\` strips to `D:`, which rejoins to
 * `D:\<id>.jpg`, so the drive root stays correct rather than becoming a special
 * case.
 */
function clipFile(clipDir: string, clip: ClipRef, ext: string): string {
  return siblingFile(clipDir, clip, `${clip.id}.${ext}`);
}

/** What locates a clip's files: its id and the sub-folder it lives in. */
export type ClipRef = Pick<ClipMeta, 'id' | 'folder'>;

/** `name` inside the clip's folder (the clips folder itself when it has none). */
function siblingFile(clipDir: string, clip: ClipRef, name: string): string {
  const base = clipDir.replace(/[\\/]+$/, '');
  const folder = (clip.folder ?? '').replace(/^[\\/]+|[\\/]+$/g, '');
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
  return convertFileSrc(clipFile(clipDir, clip, 'mp4'));
}

export function thumbUrl(clipDir: string, clip: ClipRef): string {
  return convertFileSrc(clipFile(clipDir, clip, 'jpg'));
}

/** A per-track audio file the daemon remuxed next to the clip. */
export function trackUrl(clipDir: string, clip: ClipRef, file: string): string {
  return convertFileSrc(siblingFile(clipDir, clip, file));
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
