import { MAX_AUDIO_APPS, appLabel } from './settings';
import type { GameEntry, GameProfile, ProfileApp } from './types';

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** « il y a 13 heures » for an ISO timestamp, or « jamais joué » when there is none. */
export function lastPlayedText(iso: string | null, now: number = Date.now()): string {
  if (!iso) return 'jamais joué';
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return 'jamais joué';
  return `il y a ${agoText(Math.max(0, now - then))}`;
}

/** The bare duration, « 13 heures », for `il y a …`. */
function agoText(ms: number): string {
  if (ms < MINUTE) return 'moins d’une minute';
  if (ms < HOUR) return plural(Math.floor(ms / MINUTE), 'minute', 'minutes');
  if (ms < DAY) return plural(Math.floor(ms / HOUR), 'heure', 'heures');
  if (ms < 30 * DAY) return plural(Math.floor(ms / DAY), 'jour', 'jours');
  if (ms < 365 * DAY) return `${Math.floor(ms / (30 * DAY))} mois`;
  return plural(Math.floor(ms / (365 * DAY)), 'an', 'ans');
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n > 1 ? many : one}`;
}

/** Most recently played first; never-played last, then by name. */
export function sortGames(games: GameEntry[]): GameEntry[] {
  return [...games].sort((a, b) => {
    const ta = a.last_played ? Date.parse(a.last_played) : -Infinity;
    const tb = b.last_played ? Date.parse(b.last_played) : -Infinity;
    if (ta !== tb) return tb > ta ? 1 : -1;
    return a.name.localeCompare(b.name, 'fr');
  });
}

/** The executable name from a typed value or a full path: `C:\Games\Foo.exe` becomes `Foo.exe`. */
export function exeFromInput(input: string): string {
  const base = input.trim().replace(/^["']|["']$/g, '').split(/[\\/]/).pop()?.trim() ?? '';
  if (!base) return '';
  return /\.[a-z0-9]+$/i.test(base) ? base : `${base}.exe`;
}

/** A game's display name when the daemon gave none. */
export function gameLabel(game: { exe: string; name: string }): string {
  return game.name || appLabel(game.exe);
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** The profile with `app` added, or the same object when it is already there or the list is full. */
export function withProfileApp(profile: GameProfile, app: ProfileApp): GameProfile {
  const exe = exeFromInput(app.exe);
  if (!exe || profile.apps.length >= MAX_AUDIO_APPS || profile.apps.some((a) => same(a.exe, exe))) return profile;
  return { ...profile, apps: [...profile.apps, { exe, name: app.name || appLabel(exe) }] };
}

export function withoutProfileApp(profile: GameProfile, exe: string): GameProfile {
  return { ...profile, apps: profile.apps.filter((a) => !same(a.exe, exe)) };
}

export function withMic(profile: GameProfile, mic: boolean): GameProfile {
  return { ...profile, mic };
}

/** The list with `game` replacing its entry (matched by exe), or added when new. */
export function mergeGame(games: GameEntry[], game: GameEntry): GameEntry[] {
  const i = games.findIndex((g) => same(g.exe, game.exe));
  if (i < 0) return sortGames([...games, game]);
  const next = [...games];
  next[i] = game;
  return next;
}

/** Windows not already in `taken`, filtered by a search query on name, title and exe. */
export function filterWindows<T extends { exe: string; name: string; title: string }>(
  windows: T[],
  taken: string[],
  query: string,
): T[] {
  const q = query.trim().toLowerCase();
  return windows.filter(
    (w) =>
      !taken.some((t) => same(t, w.exe)) &&
      (!q || [w.name, w.title, w.exe].some((s) => s.toLowerCase().includes(q))),
  );
}
