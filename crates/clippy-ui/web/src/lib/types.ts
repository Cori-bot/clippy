// Mirrors clippy-proto. Reimplemented rather than generated on purpose: spec §3.2
// says a UI in any language reimplements these and is in no way second-class,
// and a code generator here would be a privilege our own UI has and nobody
// else's does.

export type ClipMeta = {
  id: string;
  title: string;
  created: string;
  duration_ms: number;
  bytes: number;
  width: number;
  height: number;
  fps: number;
  encoder: string;
  has_audio: boolean;
  favorite: boolean;
  /** Stream names in file order; with more than one, index 0 is the pre-mixed "Mix". */
  audio_tracks?: string[];
  game?: string;
  /** Sub-folder of the clips folder; ''/undefined = the clips folder itself. */
  folder?: string;
};

export type LibraryPage = {
  clips: ClipMeta[];
  total: number;
  offset: number;
  /** Optional for compatibility with an older, still-running daemon. */
  total_bytes?: number;
  snapshot?: string;
};

/** Audio layout of the new MP4; the source is always preserved. */
export type ExportOutput = 'master' | 'share';
export type ExportMode = 'fast' | 'precise';
/** Actual presentation timestamps, in 100 ns units; end is exclusive. */
export type FrameIndex = { timestamps_100ns: number[]; end_100ns: number };

/**
 * Mirrors `clippy_proto::ShotMeta`. No sidecar exists, so every field is
 * recovered from the file itself, and `created` carries no UTC offset —
 * `new Date()` parses it as local time, which is the right reading for a file
 * this machine wrote.
 */
export type ShotMeta = {
  id: string;
  created: string;
  bytes: number;
  width: number;
  height: number;
};

export type Status = {
  armed: boolean;
  encoder: string | null;
  monitor_index: number;
  ring_seconds_used: number;
  ring_seconds_total: number;
  version: string;
  clip_dir: string;
  /** Replay on, idle until a game is in front. Absent from an older daemon. */
  waiting_for_game?: boolean;
  /** The program of the game in front or still running, if any. */
  game?: string | null;
  /** Display name of `game`. Absent from an older daemon. */
  game_name?: string;
  /**
   * Whether each hotkey is actually listening. `null` means the daemon has
   * not tried to bind it yet -- a real answer, and not the same as `false`.
   * Only `false` is worth putting on screen; see `hotkeyProblem` in
   * `settings.ts`.
   *
   * Optional so a status from a daemon older than this field still typechecks
   * rather than being read as `undefined` where a boolean was promised.
   */
  clip_hotkey_bound?: boolean | null;
  screenshot_hotkey_bound?: boolean | null;
};

export type Monitor = {
  index: number;
  name: string;
  width: number;
  height: number;
  left: number;
  top: number;
  adapter: string;
};

/** One row of `audio.apps`: a program with an audio session on this PC. */
export type AudioApp = { exe: string; name: string; playing: boolean };

export type Encoder = { name: string; codec: string; hardware: boolean };

export type DaemonEvent = { event: string; data: Record<string, unknown> };

/** An extra audio source of a game profile: becomes its own track in the clip. */
export type ProfileApp = { exe: string; name: string };
export type GameProfile = { apps: ProfileApp[]; mic: boolean };
export type GameEntry = {
  exe: string;
  name: string;
  enabled: boolean;
  manual: boolean;
  last_played: string | null;
  profile: GameProfile;
};
/** One row of `windows.list`: a visible top-level app window. */
export type RunningWindow = { exe: string; name: string; title: string };
