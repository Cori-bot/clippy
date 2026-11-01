import { describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { diag, diagEnabled, summarizeArgs } from './diag';

describe('diagnostics', () => {
  it('is off until the setting turns it on, and recording is then a no-op', () => {
    expect(diagEnabled()).toBe(false);
    expect(() => diag('click', { label: 'x' })).not.toThrow();
  });

  it('trims long string arguments and leaves the rest alone', () => {
    const out = summarizeArgs({ clip_id: 'a', path: 'x'.repeat(300), start_ms: 5, tracks: [1] });
    expect(out.clip_id).toBe('a');
    expect(out.start_ms).toBe(5);
    expect(out.tracks).toEqual([1]);
    expect((out.path as string).length).toBe(121);
  });
});
