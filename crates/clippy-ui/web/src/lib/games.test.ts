import { describe, expect, it } from 'vitest';
import { exeFromInput, filterWindows, lastPlayedText, mergeGame, sortGames, withMic, withProfileApp, withoutProfileApp } from './games';
import type { GameEntry, GameProfile } from './types';

const NOW = Date.parse('2026-10-20T12:00:00Z');
const ago = (ms: number) => new Date(NOW - ms).toISOString();
const H = 3_600_000;
const D = 24 * H;

const game = (exe: string, last: string | null = null): GameEntry => ({
  exe, name: exe.replace('.exe', ''), enabled: true, manual: false, last_played: last, profile: { apps: [], mic: false },
});

describe('lastPlayedText', () => {
  it('says never when there is no date', () => {
    expect(lastPlayedText(null, NOW)).toBe('jamais joué');
    expect(lastPlayedText('garbage', NOW)).toBe('jamais joué');
  });
  it('picks the unit and pluralises', () => {
    expect(lastPlayedText(ago(10_000), NOW)).toBe('il y a moins d’une minute');
    expect(lastPlayedText(ago(5 * 60_000), NOW)).toBe('il y a 5 minutes');
    expect(lastPlayedText(ago(H), NOW)).toBe('il y a 1 heure');
    expect(lastPlayedText(ago(13 * H), NOW)).toBe('il y a 13 heures');
    expect(lastPlayedText(ago(2 * D), NOW)).toBe('il y a 2 jours');
    expect(lastPlayedText(ago(95 * D), NOW)).toBe('il y a 3 mois');
    expect(lastPlayedText(ago(800 * D), NOW)).toBe('il y a 2 ans');
  });
  it('clamps a date in the future', () => {
    expect(lastPlayedText(ago(-H), NOW)).toBe('il y a moins d’une minute');
  });
});

describe('sortGames', () => {
  it('orders by recency, never-played last by name', () => {
    const list = sortGames([game('b.exe'), game('old.exe', ago(D)), game('a.exe'), game('new.exe', ago(H))]);
    expect(list.map((g) => g.exe)).toEqual(['new.exe', 'old.exe', 'a.exe', 'b.exe']);
  });
});

describe('exeFromInput', () => {
  it('keeps the file name and adds .exe when missing', () => {
    expect(exeFromInput('  C:\\Games\\Foo.exe ')).toBe('Foo.exe');
    expect(exeFromInput('"D:/x/bar"')).toBe('bar.exe');
    expect(exeFromInput('   ')).toBe('');
  });
});

describe('profile helpers', () => {
  const empty: GameProfile = { apps: [], mic: false };
  it('adds an app once, case-insensitively', () => {
    const one = withProfileApp(empty, { exe: 'Spotify.exe', name: 'Spotify' });
    expect(one.apps).toHaveLength(1);
    expect(withProfileApp(one, { exe: 'spotify.exe', name: '' })).toBe(one);
  });
  it('refuses an empty exe and a full list', () => {
    expect(withProfileApp(empty, { exe: ' ', name: '' })).toBe(empty);
    let p = empty;
    for (let i = 0; i < 12; i++) p = withProfileApp(p, { exe: `a${i}.exe`, name: '' });
    expect(p.apps).toHaveLength(8);
  });
  it('removes and toggles the mic', () => {
    const p = withProfileApp(empty, { exe: 'a.exe', name: 'A' });
    expect(withoutProfileApp(p, 'A.EXE').apps).toEqual([]);
    expect(withMic(p, true).mic).toBe(true);
    expect(p.mic).toBe(false);
  });
});

describe('mergeGame / filterWindows', () => {
  it('replaces by exe or adds', () => {
    const a = game('a.exe');
    expect(mergeGame([a], { ...a, enabled: false })[0].enabled).toBe(false);
    expect(mergeGame([a], game('b.exe'))).toHaveLength(2);
  });
  it('hides added games and filters by query', () => {
    const w = [{ exe: 'a.exe', name: 'A', title: 'x' }, { exe: 'b.exe', name: 'Beta', title: 'Level 1' }, { exe: 'c.exe', name: 'C', title: '' }];
    expect(filterWindows(w, ['A.exe'], '').map((x) => x.exe)).toEqual(['b.exe', 'c.exe']);
    expect(filterWindows(w, [], 'level').map((x) => x.exe)).toEqual(['b.exe']);
  });
});
