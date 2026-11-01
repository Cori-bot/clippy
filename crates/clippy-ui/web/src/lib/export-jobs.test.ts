import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ExportJobStore, type ExportJobSnapshot, type ExportStartArgs } from './export-jobs';
import type { ClipMeta } from './types';

const args: ExportStartArgs = { clip_id: 'source', start_ms: 0, end_ms: 2000, mode: 'fast', output: 'share' };
const clip: ClipMeta = { id: 'saved', title: 'Saved', created: '2026-10-26', duration_ms: 2000, bytes: 2000, width: 160, height: 96, fps: 10, encoder: 'H.264', has_audio: true, favorite: false };
const job = (state: ExportJobSnapshot['state'] = 'running', percent = 10, id = 'job-1'): ExportJobSnapshot => ({ job_id: id, state, percent, stage: 'writing', ...(state === 'completed' ? {clip} : {}) });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
let stores: ExportJobStore[];
function store(request = vi.fn(), completed = vi.fn()) {
  let token: string | undefined;
  const value = new ExportJobStore(async (command, payload) => {
    if (command === 'library.export.start') token = payload?.request_token as string;
    const result = await request(command, payload);
    return result && typeof result === 'object' && 'job_id' in result
      ? { request_token: token, ...result } : result;
  }, completed);
  stores.push(value);
  return { value, request, completed };
}
beforeEach(() => { vi.useFakeTimers(); stores = []; });
afterEach(() => { stores.forEach(value => value.disconnect()); vi.useRealTimers(); });

describe('daemon-owned export jobs', () => {
  it('preserves precise mode and a single-frame selection in the request', async () => {
    const {value, request} = store(vi.fn().mockResolvedValue(job('queued', 0)));
    await value.start({...args, mode: 'precise', start_ms: 17, end_ms: 33});
    expect(request).toHaveBeenCalledWith('library.export.start', expect.objectContaining({mode: 'precise', start_ms: 17, end_ms: 33}));
  });
  it('locks before awaiting start and keeps the output and intentional mute choices', async () => {
    const waiting = deferred<ExportJobSnapshot>();
    const {value,request} = store(vi.fn().mockReturnValue(waiting.promise));
    const first = value.start({...args,tracks: []});
    await value.start(args);
    expect(request).toHaveBeenCalledTimes(1);
    expect(value.starting).toBe(true);
    expect(value.active).toBe(true);
    expect(request).toHaveBeenCalledWith('library.export.start', {...args,tracks: [],request_token:expect.any(String)});
    waiting.resolve(job('queued',0));
    await first;
    expect(value.starting).toBe(false);
    expect(value.job?.state).toBe('queued');
  });

  it('polls progress then completes once, leaving no polling timer', async () => {
    const {value,request,completed} = store(vi.fn()
      .mockResolvedValueOnce(job('queued',0))
      .mockResolvedValueOnce(job('running',45))
      .mockResolvedValue(job('completed',100)));
    await value.start(args);
    await vi.advanceTimersByTimeAsync(350);
    expect(value.job?.percent).toBe(45);
    expect(value.active).toBe(true);
    await vi.advanceTimersByTimeAsync(350);
    expect(value.active).toBe(false);
    expect(value.job?.clip).toEqual(clip);
    expect(completed).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
    await value.resume();
    expect(completed).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('library.export.status', {job_id:'job-1'});
  });

  it('waits for cancellation acknowledgement and rejects duplicate cancel requests', async () => {
    const cancellation = deferred<ExportJobSnapshot>();
    const {value,request} = store(vi.fn().mockResolvedValueOnce(job()).mockReturnValueOnce(cancellation.promise).mockResolvedValue(job('cancelled',30)));
    await value.start(args);
    const pending = value.cancel();
    await value.cancel();
    expect(value.cancelling).toBe(true);
    expect(value.job?.state).toBe('running');
    expect(request).toHaveBeenCalledTimes(2);
    cancellation.resolve(job('cancelling',30));
    await pending;
    expect(value.job?.state).toBe('cancelling');
    await vi.advanceTimersByTimeAsync(350);
    expect(value.job?.state).toBe('cancelled');
    expect(value.active).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not let an older running response replace a terminal cancel response', async () => {
    const status = deferred<ExportJobSnapshot>();
    const {value} = store(vi.fn().mockResolvedValueOnce(job()).mockReturnValueOnce(status.promise).mockResolvedValueOnce(job('cancelled',40)));
    await value.start(args);
    await vi.advanceTimersByTimeAsync(350);
    await value.cancel();
    status.resolve(job('running',20));
    await Promise.resolve(); await Promise.resolve();
    expect(value.job?.state).toBe('cancelled');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('ignores an old status reply after disconnect and resumes a newer job', async () => {
    const old = deferred<ExportJobSnapshot>();
    const {value,request} = store(vi.fn().mockResolvedValueOnce(job()).mockReturnValueOnce(old.promise).mockResolvedValueOnce(job('running',60,'job-2')));
    await value.start(args);
    await vi.advanceTimersByTimeAsync(350);
    value.disconnect();
    expect(vi.getTimerCount()).toBe(0);
    await value.resume();
    old.resolve(job('failed',10));
    await Promise.resolve(); await Promise.resolve();
    expect(value.job?.job_id).toBe('job-2');
    expect(value.error).toBeNull();
    expect(request).toHaveBeenLastCalledWith('library.export.status', {});
  });

  it('ignores a start response from a disconnected session', async () => {
    const old = deferred<ExportJobSnapshot>();
    const {value} = store(vi.fn().mockReturnValueOnce(old.promise).mockResolvedValueOnce(null));
    const start = value.start(args);
    value.disconnect();
    await value.resume();
    old.resolve(job());
    await start;
    expect(value.job).toBeNull();
    expect(value.starting).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('clears an obsolete cancellation request when resuming a fresh status', async () => {
    const cancel = deferred<ExportJobSnapshot>();
    const {value} = store(vi.fn().mockResolvedValueOnce(job()).mockReturnValueOnce(cancel.promise).mockResolvedValueOnce(job('running',40)));
    await value.start(args);
    const pending = value.cancel();
    expect(value.cancelling).toBe(true);
    await value.resume();
    expect(value.cancelling).toBe(false);
    cancel.resolve(job('cancelled',20));
    await pending;
    expect(value.job?.state).toBe('running');
    expect(value.job?.percent).toBe(40);
  });

  it('retains daemon ownership during disconnect and releases a lost job after restart', async () => {
    const {value} = store(vi.fn().mockResolvedValueOnce(job()).mockResolvedValueOnce(null));
    await value.start(args);
    value.disconnect();
    expect(value.active).toBe(true);
    await value.resume();
    expect(value.active).toBe(false);
    expect(value.error).toContain('redémarrage');
  });

  it('shows polling failure and retries the same job without starting another export', async () => {
    const {value,request} = store(vi.fn().mockResolvedValueOnce(job()).mockRejectedValueOnce(new Error('pipe fermé')).mockResolvedValueOnce(job('completed',100)));
    await value.start(args);
    await vi.advanceTimersByTimeAsync(350);
    expect(value.error).toContain('pipe fermé');
    expect(value.active).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(value.job?.state).toBe('completed');
    expect(value.error).toBeNull();
    expect(request.mock.calls.filter(call => call[0] === 'library.export.start')).toHaveLength(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('surfaces failed, missing and malformed status instead of announcing success', async () => {
    const {value,completed} = store(vi.fn().mockResolvedValueOnce({...job('failed',20),error:'Encodeur indisponible'}));
    await value.start(args);
    expect(value.error).toBe('Encodeur indisponible');
    expect(value.active).toBe(false);
    expect(completed).not.toHaveBeenCalled();
    const missing = store(vi.fn().mockResolvedValueOnce(job()).mockResolvedValueOnce(null)).value;
    await missing.start(args);
    await vi.advanceTimersByTimeAsync(350);
    expect(missing.error).toContain('ne retrouve plus');
    expect(missing.active).toBe(false);
    const malformed = store(vi.fn().mockResolvedValueOnce({job_id:'x',state:'completed',percent:100,stage:'done'}).mockResolvedValueOnce(null)).value;
    await malformed.start(args);
    expect(malformed.error).toContain('invalide');
    expect(malformed.active).toBe(false);
  });

  it('keeps cancellation failures visible and polls for the authoritative outcome', async () => {
    const {value} = store(vi.fn().mockResolvedValueOnce(job()).mockRejectedValueOnce(new Error('déconnecté')).mockResolvedValueOnce(job('completed',100)));
    await value.start(args);
    await value.cancel();
    expect(value.error).toContain('Annulation non confirmée');
    expect(value.job?.state).toBe('running');
    await vi.advanceTimersByTimeAsync(350);
    expect(value.job?.state).toBe('completed');
  });

  it('dismisses terminal jobs without resurrecting the same latest result on resume', async () => {
    const {value} = store(vi.fn().mockResolvedValue(job('completed',100)));
    await value.resume();
    value.dismiss();
    expect(value.job).toBeNull();
    await value.resume();
    expect(value.job).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('cannot dismiss active work or launch an export while disconnected', async () => {
    const {value,request} = store(vi.fn().mockResolvedValue(job()));
    await value.start(args);
    value.dismiss();
    expect(value.job).not.toBeNull();
    value.disconnect();
    const fresh = store();
    fresh.value.disconnect();
    await fresh.value.start(args);
    expect(fresh.request).not.toHaveBeenCalled();
    expect(fresh.value.error).toContain('déconnecté');
    expect(request).toHaveBeenCalledTimes(1);
  });
});

describe('uncertain export ownership', () => {
  it('does not attribute an older completed export to a rejected new source', async () => {
    const completed = vi.fn();
    const request = vi.fn(async (command: string, payload?: Record<string, unknown>) => {
      if (command === 'library.export.start') throw new Error('source invalide');
      return payload?.request_token ? null : job('completed',100,'old-job');
    });
    const value = new ExportJobStore(request,completed); stores.push(value);
    await value.start(args);
    expect(value.job).toBeNull();
    expect(value.error).toContain('source invalide');
    expect(completed).not.toHaveBeenCalled();
    expect(request).toHaveBeenLastCalledWith('library.export.status', {request_token:expect.any(String)});
  });

  it('retains an uncertain recipe token for retries and rotates it after a terminal result', async () => {
    const tokens: string[] = [];
    let starts = 0;
    const request = vi.fn(async (command: string, payload?: Record<string, unknown>) => {
      if (command === 'library.export.status') return null;
      tokens.push(payload!.request_token as string);
      if (++starts === 1) throw new Error('réponse perdue');
      return {...job('failed'),request_token:payload!.request_token,error:'encodeur'};
    });
    const value = new ExportJobStore(request); stores.push(value);
    await value.start(args);
    expect(value.active).toBe(false);
    await value.start({...args});
    expect(tokens[1]).toBe(tokens[0]);
    await value.start(args);
    expect(tokens[2]).not.toBe(tokens[1]);
  });

  it('rejects direct and recovered snapshots belonging to another request', async () => {
    const completed = vi.fn();
    const request = vi.fn().mockResolvedValue({...job('completed',100),request_token:'another-request'});
    const value = new ExportJobStore(request,completed); stores.push(value);
    await value.start(args);
    expect(value.recovering).toBe(true);
    expect(value.error).toContain('autre demande');
    expect(completed).not.toHaveBeenCalled();
  });

  it('blocks starting while reconnect status is unresolved', async () => {
    const status = deferred<ExportJobSnapshot | null>();
    const {value,request} = store(vi.fn().mockReturnValueOnce(status.promise));
    const recovering = value.resume();
    expect(value.recovering).toBe(true);
    expect(value.active).toBe(true);
    await value.start(args);
    expect(request).toHaveBeenCalledTimes(1);
    status.resolve(job('completed',100));
    await recovering;
    expect(value.recovering).toBe(false);
    expect(value.active).toBe(false);
    expect(value.job?.state).toBe('completed');
  });

  it.each(['running','completed'] as const)('recovers a %s job after losing its start response, without another start', async state => {
    const status = deferred<ExportJobSnapshot>();
    const {value,request} = store(vi.fn().mockRejectedValueOnce(new Error('réponse perdue')).mockReturnValueOnce(status.promise));
    const starting = value.start(args);
    await Promise.resolve(); await Promise.resolve();
    expect(value.active).toBe(true);
    await value.start(args);
    expect(request).toHaveBeenCalledTimes(2);
    expect(request).toHaveBeenLastCalledWith('library.export.status', {request_token:expect.any(String)});
    status.resolve(job(state,state === 'completed' ? 100 : 20));
    await starting;
    expect(value.job?.state).toBe(state);
    expect(value.error).toBeNull();
    expect(request.mock.calls.filter(call => call[0] === 'library.export.start')).toHaveLength(1);
  });

  it('keeps ownership blocked after recovery errors until a successful retry', async () => {
    const {value,request} = store(vi.fn().mockRejectedValueOnce(new Error('statut indisponible')).mockResolvedValueOnce(null).mockResolvedValueOnce(job('queued',0)));
    await value.resume();
    expect(value.error).toContain('statut indisponible');
    expect(value.recovering).toBe(true);
    value.dismiss();
    await value.start(args);
    expect(request).toHaveBeenCalledTimes(1);
    await value.resume();
    expect(value.active).toBe(false);
    expect(value.error).toBeNull();
    await value.start(args);
    expect(request).toHaveBeenLastCalledWith('library.export.start', {...args,request_token:expect.any(String)});
  });

  it('recovers a lost start only after failed status requests can be retried', async () => {
    const {value,request} = store(vi.fn().mockRejectedValueOnce(new Error('réponse perdue')).mockRejectedValueOnce(new Error('moteur inaccessible')).mockResolvedValueOnce(job('completed',100)));
    await value.start(args);
    expect(value.active).toBe(true);
    expect(value.error).toContain('moteur inaccessible');
    await value.start(args);
    expect(request).toHaveBeenCalledTimes(2);
    await value.resume();
    expect(value.recovering).toBe(false);
    expect(value.job?.state).toBe('completed');
    expect(value.active).toBe(false);
  });

  it('a successful known-job poll clears a failed recovery gate', async () => {
    const {value} = store(vi.fn().mockResolvedValueOnce(job()).mockRejectedValueOnce(new Error('statut momentanément indisponible')).mockResolvedValueOnce(job('running',45)));
    await value.start(args);
    await value.resume();
    expect(value.recovering).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(value.recovering).toBe(false);
    expect(value.job?.percent).toBe(45);
    expect(value.error).toBeNull();
  });
});
