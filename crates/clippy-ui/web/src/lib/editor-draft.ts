import { useEffect, useRef, useState } from 'react';
import { defaultStates, type TrackState } from './tracks';

export type EditorSource = { directory: string; id: string; durationMs: number; audio: string[] };
export type EditRecipe = { inMs: number; outMs: number; tracks: TrackState[] };
export type EditHistory = { past: EditRecipe[]; present: EditRecipe; future: EditRecipe[]; group: string; at: number };
const VERSION = 1;
const HISTORY_LIMIT = 50;
const GROUP_GAP_MS = 450;

export function draftKey(source: EditorSource): string {
  return `clippy:edit:${JSON.stringify([source.directory.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase(), source.id])}`;
}
function signature(source: EditorSource): string { return JSON.stringify([source.durationMs, source.audio]); }
export function originalRecipe(source: EditorSource): EditRecipe {
  return { inMs: 0, outMs: source.durationMs, tracks: defaultStates(Math.max(0, source.audio.length - 1)) };
}
export function serializeDraft(source: EditorSource, recipe: EditRecipe): string {
  return JSON.stringify({ version: VERSION, signature: signature(source), recipe });
}
/** Reject outdated source metadata and corrupt values rather than applying another mix. */
export function parseDraft(source: EditorSource, raw: string | null): EditRecipe | null {
  if (!raw) return null;
  try {
    const data = JSON.parse(raw);
    if (data.version !== VERSION || data.signature !== signature(source)) return null;
    const r = data.recipe;
    if (!r || !Number.isFinite(r.inMs) || !Number.isFinite(r.outMs)
      || r.inMs < 0 || r.inMs >= source.durationMs + 1 || r.outMs < 0 || r.outMs >= source.durationMs + 1
      || !Array.isArray(r.tracks) || r.tracks.length !== Math.max(0, source.audio.length - 1)) return null;
    if (!r.tracks.every((t: TrackState) => t && typeof t.muted === 'boolean'
      && Number.isInteger(t.gain) && t.gain >= 0 && t.gain <= 200)) return null;
    return { inMs: r.inMs, outMs: r.outMs, tracks: r.tracks.map((t: TrackState) => ({ muted: t.muted, gain: t.gain })) };
  } catch { return null; }
}
export function sameRecipe(a: EditRecipe, b: EditRecipe): boolean {
  return a.inMs === b.inMs && a.outMs === b.outMs && a.tracks.length === b.tracks.length
    && a.tracks.every((t, i) => t.muted === b.tracks[i].muted && t.gain === b.tracks[i].gain);
}
export function initialHistory(recipe: EditRecipe): EditHistory {
  return { past: [], present: recipe, future: [], group: '', at: 0 };
}
export function changeRecipe(history: EditHistory, next: EditRecipe, group: string, at: number): EditHistory {
  if (sameRecipe(history.present, next)) return history;
  const grouped = group !== '' && group === history.group && at >= history.at && at - history.at < GROUP_GAP_MS;
  return {
    past: grouped ? history.past : [...history.past, history.present].slice(-HISTORY_LIMIT),
    present: next, future: [], group, at,
  };
}
export function undoRecipe(h: EditHistory): EditHistory {
  if (!h.past.length) return h;
  return { past: h.past.slice(0, -1), present: h.past[h.past.length - 1], future: [h.present, ...h.future].slice(0, HISTORY_LIMIT), group: '', at: 0 };
}
export function redoRecipe(h: EditHistory): EditHistory {
  if (!h.future.length) return h;
  return { past: [...h.past, h.present].slice(-HISTORY_LIMIT), present: h.future[0], future: h.future.slice(1), group: '', at: 0 };
}

type Session = { identity: string; history: EditHistory; status: 'saved' | 'pending' | 'original' | 'unavailable' | 'discarded' };
function loadSession(source: EditorSource): Session {
  const identity = `${draftKey(source)}:${signature(source)}`;
  try {
    const raw = localStorage.getItem(draftKey(source));
    const restored = parseDraft(source, raw);
    return { identity, history: initialHistory(restored ?? originalRecipe(source)), status: restored ? 'saved' : raw ? 'discarded' : 'original' };
  } catch {
    return { identity, history: initialHistory(originalRecipe(source)), status: 'unavailable' };
  }
}

/** Saves only user commands; opening/switching clips never overwrites their draft. */
export function useEditorDraft(source: EditorSource) {
  const identity = `${draftKey(source)}:${signature(source)}`;
  const [stored, setStored] = useState(() => loadSession(source));
  let session = stored;
  if (stored.identity !== identity) {
    session = loadSession(source);
    setStored(session);
  }
  const current = useRef(session);
  current.current = session;
  const pending = useRef<{ key: string; value: string; identity: string } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const flush = () => {
    clearTimeout(timer.current);
    const write = pending.current;
    pending.current = null;
    if (!write) return;
    let status: Session['status'] = 'saved';
    try { localStorage.setItem(write.key, write.value); }
    catch { status = 'unavailable'; }
    if (current.current.identity === write.identity) {
      const next = { ...current.current, status };
      current.current = next;
      setStored(next);
    }
  };
  useEffect(() => {
    window.addEventListener('pagehide', flush);
    return () => {
      window.removeEventListener('pagehide', flush);
      flush();
    };
    // Flush captures only refs; flush the previous recipe before loading another.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identity]);
  const commit = (history: EditHistory) => {
    if (history === current.current.history) return;
    // If a command arrives immediately after a clip switch, preserve the old
    // pending write before replacing it with the new clip's recipe.
    if (pending.current && pending.current.identity !== identity) flush();
    pending.current = { key: draftKey(source), value: serializeDraft(source, history.present), identity };
    clearTimeout(timer.current);
    timer.current = setTimeout(flush, 250);
    const next: Session = { identity, history, status: 'pending' };
    current.current = next;
    setStored(next);
  };
  return {
    recipe: session.history.present,
    status: session.status,
    canUndo: session.history.past.length > 0,
    canRedo: session.history.future.length > 0,
    changed: !sameRecipe(session.history.present, originalRecipe(source)),
    update: (next: EditRecipe, group: string) => commit(changeRecipe(current.current.history, next, group, Date.now())),
    undo: () => commit(undoRecipe(current.current.history)),
    redo: () => commit(redoRecipe(current.current.history)),
    reset: () => commit(changeRecipe(current.current.history, originalRecipe(source), '', Date.now())),
  };
}
