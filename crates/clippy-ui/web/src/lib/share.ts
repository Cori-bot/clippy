/**
 * Size caps for a share export. The daemon (`export::share_plan`) chooses the
 * bitrate and picture size; this side only names the caps and refuses early a
 * selection the daemon would refuse anyway.
 */
export type SizePreset = { mb: number; label: string; description: string };

export const SIZE_PRESETS: SizePreset[] = [
  { mb: 20, label: '20 Mo · Discord', description: 'Compte Discord gratuit.' },
  { mb: 50, label: '50 Mo · Nitro Basic', description: 'Discord Nitro Basic.' },
  { mb: 500, label: '500 Mo · Nitro', description: 'Discord Nitro.' },
];

// Mirrors `precise.rs`: 90 % of the cap, one 192 kb/s AAC mix, 150 kb/s video floor.
const HEADROOM = 0.9;
const AUDIO_BPS = 192_000;
const MIN_VIDEO_BPS = 150_000;

/** Longest selection, in milliseconds, the daemon accepts for a `mb` cap. */
export function longestForCap(mb: number, withAudio: boolean): number {
  const bits = mb * 1_000_000 * 8 * HEADROOM;
  return Math.floor((bits / (MIN_VIDEO_BPS + (withAudio ? AUDIO_BPS : 0))) * 1000);
}

/** Why a selection cannot fit the cap, or null when it can. */
export function capError(mb: number | null, durationMs: number, withAudio: boolean): string | null {
  if (mb === null) return null;
  const longest = longestForCap(mb, withAudio);
  if (durationMs <= longest) return null;
  return `Trop long pour ${mb} Mo : gardez au plus ${Math.floor(longest / 1000)} s.`;
}

/**
 * The 9:16 band, in the same units as `width`/`height` (rendered pixels), a
 * `center`/1000 crop keeps. Mirrors `export::vertical_crop` minus the even
 * rounding the encoder needs.
 */
export function verticalBand(width: number, height: number, center: number): { left: number; width: number } {
  const band = Math.min(width, (height * 9) / 16);
  const left = Math.min(Math.max(0, (width * Math.min(1000, Math.max(0, center))) / 1000 - band / 2), width - band);
  return { left, width: band };
}
