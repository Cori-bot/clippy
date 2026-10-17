import { convertFileSrc } from '@tauri-apps/api/core';

/**
 * Screenshots live in a `Screenshots` subfolder of the clips folder, because
 * `{clip_dir}\{id}.jpg` is already a *clip's* thumbnail — a screenshot written
 * beside the clips under the same id grammar would overwrite one.
 *
 * The trailing separator is stripped for the reason `clips.ts` documents:
 * `clip_dir` is a config key a person types, a drive root (`D:\`) carries one
 * by necessity, and the doubled separator that results can miss the
 * asset-scope glob Tauri builds from the same directory — which shows as a
 * broken image with nothing logged and nothing on screen to say why.
 */
function shotFile(clipDir: string, id: string, suffix: string): string {
  return `${clipDir.replace(/[\\/]+$/, '')}\\Screenshots\\${id}${suffix}`;
}

/**
 * When a screenshot was taken, written the French way ("26 août 2026 à 14:30:12").
 * One helper so the tile, the viewer's alt text and the delete dialog cannot
 * word the same instant three different ways. `timeZone` is only for tests.
 */
export function formatShotDate(created: string, timeZone?: string): string {
  return new Date(created).toLocaleString('fr-FR', { dateStyle: 'long', timeStyle: 'medium', timeZone });
}

/** The full-resolution screenshot. */
export function shotUrl(clipDir: string, id: string): string {
  return convertFileSrc(shotFile(clipDir, id, '.jpg'));
}

/**
 * The 640 px copy the grid draws.
 *
 * Not an optimisation to skip: fifty full-size screenshots decode to roughly
 * 450 MB of bitmap in the webview, the measured hazard `thumb.rs` already
 * records for the clip grid.
 */
export function shotThumbUrl(clipDir: string, id: string): string {
  return convertFileSrc(shotFile(clipDir, id, '.thumb.jpg'));
}
