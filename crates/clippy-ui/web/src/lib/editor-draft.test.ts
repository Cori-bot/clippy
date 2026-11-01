import { describe, expect, it } from 'vitest';
import { changeRecipe, draftKey, initialHistory, originalRecipe, parseDraft, redoRecipe, serializeDraft, undoRecipe, type EditorSource } from './editor-draft';
const source: EditorSource = { directory: 'D:\\Clips\\', id: 'clip1', durationMs: 30000, audio: ['Mix', 'Jeu', 'Micro'] };
const recipe = { inMs: 2000, outMs: 27000, tracks: [{ muted: false, gain: 175 }, { muted: true, gain: 50 }] };

describe('non-destructive editor recipes', () => {
  it('preserves a real fractional final frame boundary beyond truncated metadata', () => {
    const short = {...source, durationMs: 66};
    const precise = {...recipe, inMs: 16.6667, outMs: 66.6667};
    expect(parseDraft(short, serializeDraft(short, precise))).toEqual(precise);
    expect(parseDraft(short, serializeDraft(short, {...precise, outMs: 67}))).toBeNull();
  });
  it('round trips trim and full track gain/mute without changing source metadata', () => {
    const before = JSON.stringify(source);
    expect(parseDraft(source, serializeDraft(source, recipe))).toEqual(recipe);
    expect(JSON.stringify(source)).toBe(before);
  });
  it('keeps directories isolated and normalizes Windows path spelling', () => {
    expect(draftKey(source)).toBe(draftKey({ ...source, directory: 'd:/clips' }));
    expect(draftKey(source)).not.toBe(draftKey({ ...source, directory: 'E:/clips' }));
    expect(draftKey(source)).not.toBe(draftKey({ ...source, id: 'clip2' }));
  });
  it('rejects changed duration, track names/order, or schema version', () => {
    const raw = serializeDraft(source, recipe);
    expect(parseDraft({ ...source, durationMs: 10000 }, raw)).toBeNull();
    expect(parseDraft({ ...source, audio: ['Mix', 'Micro', 'Jeu'] }, raw)).toBeNull();
    expect(parseDraft(source, raw.replace('"version":1', '"version":2'))).toBeNull();
  });
  it.each([null, '{', 'null', '{}', '[1]'])('rejects corrupt storage %s', (raw) => {
    expect(parseDraft(source, raw)).toBeNull();
  });
  it('rejects ranges outside the source, non-finite endpoints and mismatched track state', () => {
    for (const bad of [
      { ...recipe, inMs: -1 }, { ...recipe, outMs: 30001 }, { ...recipe, inMs: 30001 },
      { ...recipe, inMs: Infinity }, { ...recipe, tracks: [] },
      { ...recipe, tracks: [{ muted: false, gain: 201 }, recipe.tracks[1]] },
      { ...recipe, tracks: [{ muted: false, gain: 1.5 }, recipe.tracks[1]] },
    ]) expect(parseDraft(source, serializeDraft(source, bad))).toBeNull();
  });
  it('preserves untrimmable old clips with no editable tracks', () => {
    const old = { ...source, durationMs: 0, audio: ['Mix'] };
    expect(parseDraft(old, serializeDraft(old, originalRecipe(old)))).toEqual({ inMs: 0, outMs: 0, tracks: [] });
  });
  it('preserves an unfinished crossed range so the editor can show its export error after reopening', () => {
    const unfinished = { ...recipe, inMs: 28000, outMs: 27000 };
    expect(parseDraft(source, serializeDraft(source, unfinished))).toEqual(unfinished);
  });
});

describe('bounded grouped editor undo', () => {
  it('groups consecutive slider changes, then restores the pre-gesture recipe', () => {
    const initial = originalRecipe(source);
    let h = initialHistory(initial);
    h = changeRecipe(h, { ...initial, inMs: 100 }, 'trim', 1000);
    h = changeRecipe(h, { ...initial, inMs: 200 }, 'trim', 1100);
    h = changeRecipe(h, { ...initial, inMs: 300 }, 'trim', 1200);
    expect(h.past).toHaveLength(1);
    const undone = undoRecipe(h);
    expect(undone.present).toEqual(initial);
    expect(redoRecipe(undone).present.inMs).toBe(300);
  });
  it('separates control changes and gestures after an idle gap', () => {
    let h = initialHistory(originalRecipe(source));
    h = changeRecipe(h, { ...h.present, inMs: 100 }, 'trim', 1000);
    h = changeRecipe(h, { ...h.present, inMs: 200 }, 'trim', 1500);
    h = changeRecipe(h, recipe, 'track:0', 1550);
    expect(h.past).toHaveLength(3);
  });
  it('new edits after undo discard redo and start a new group', () => {
    let h = changeRecipe(initialHistory(originalRecipe(source)), recipe, 'trim', 1000);
    h = undoRecipe(h);
    h = changeRecipe(h, { ...h.present, inMs: 300 }, 'trim', 1100);
    expect(h.future).toEqual([]);
    expect(undoRecipe(h).present).toEqual(originalRecipe(source));
  });
  it('bounds history to 50 snapshots and treats reset as one undoable command', () => {
    let h = initialHistory(originalRecipe(source));
    for (let i = 1; i <= 80; i++) h = changeRecipe(h, { ...h.present, inMs: i }, 'trim', i * 1000);
    expect(h.past).toHaveLength(50);
    h = changeRecipe(h, originalRecipe(source), '', 81000);
    expect(undoRecipe(h).present.inMs).toBe(80);
  });
  it('ignores no-op changes and empty undo/redo', () => {
    const h = initialHistory(recipe);
    expect(changeRecipe(h, { ...recipe }, 'trim', 1000)).toBe(h);
    expect(undoRecipe(h)).toBe(h);
    expect(redoRecipe(h)).toBe(h);
  });
});
