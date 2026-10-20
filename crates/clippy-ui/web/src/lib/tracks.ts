/**
 * Pure logic behind the per-track audio rows of the clip editor, kept out of
 * the components so it can be tested without a DOM.
 *
 * A clip recorded with several sources carries `audio_tracks` (names in file
 * stream order). When there is more than one, stream 0 is the pre-mixed "Mix"
 * and the editable sources are streams 1... A clip with one stream or none has
 * nothing to edit and keeps the plain `<video>` behaviour.
 */
import { clamp } from './ui';

export type EditableTrack = { index: number; name: string };

/** What the user chose for one track. `gain` is a whole percent, 0..200. */
export type TrackState = { muted: boolean; gain: number };

export type ExportTrack = { index: number; gain: number };

export const DEFAULT_GAIN = 100;
export const MAX_GAIN = 200;
/** Above this a media element cannot go: `audio.volume` stops at 1.0. */
export const PREVIEW_MAX_GAIN = 100;
/** How far an audio element may drift from the video before it is re-synced. */
export const DRIFT_LIMIT_S = 0.08;

/** The tracks the user can edit: every stream after the Mix. */
export function editableTracks(audioTracks: string[] | undefined): EditableTrack[] {
  if (!audioTracks || audioTracks.length <= 1) return [];
  return audioTracks.slice(1).map((name, i) => ({ index: i + 1, name }));
}

export function defaultStates(count: number): TrackState[] {
  return Array.from({ length: count }, () => ({ muted: false, gain: DEFAULT_GAIN }));
}

export function clampGain(gain: number): number {
  return Number.isFinite(gain) ? clamp(Math.round(gain), 0, MAX_GAIN) : DEFAULT_GAIN;
}

/** True when any track is muted or off its default level. */
export function tracksChanged(states: TrackState[]): boolean {
  return states.some((s) => s.muted || clampGain(s.gain) !== DEFAULT_GAIN);
}

/**
 * The `tracks` argument of `library.export`: the tracks to keep with their
 * gain. `undefined` when nothing changed, so the request stays exactly what it
 * was before per-track editing existed.
 */
export function buildExportTracks(
  tracks: EditableTrack[],
  states: TrackState[],
): ExportTrack[] | undefined {
  if (!tracksChanged(states)) return undefined;
  const kept: ExportTrack[] = [];
  tracks.forEach((t, i) => {
    const s = states[i];
    if (s && !s.muted) kept.push({ index: t.index, gain: clampGain(s.gain) });
  });
  return kept;
}

/** Whether an audio element has drifted far enough from the video to be re-synced. */
export function needsResync(audioTime: number, videoTime: number, limit = DRIFT_LIMIT_S): boolean {
  return Math.abs(audioTime - videoTime) > limit;
}

/**
 * The element volume (0..1) for one track: its own level times the master.
 * Capped at 1 -- levels above 100% only exist in the export.
 */
export function trackVolume(state: TrackState, masterLevel: number, masterMuted: boolean): number {
  if (state.muted || masterMuted) return 0;
  const own = clamp(clampGain(state.gain), 0, PREVIEW_MAX_GAIN) / 100;
  return clamp((masterLevel / 100) * own, 0, 1);
}
