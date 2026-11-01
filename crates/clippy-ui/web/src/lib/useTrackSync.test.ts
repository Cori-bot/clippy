import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Exercise the actual hook effect with controlled browser APIs, including a
// resume promise that never resolves. No DOM renderer or audio device needed.
const hooks = vi.hoisted(() => ({ effects: [] as (() => void | (() => void))[], state: vi.fn() }));
vi.mock('react', () => ({
  useEffect: (effect: () => void | (() => void)) => hooks.effects.push(effect),
  useRef: (value: unknown) => ({ current: value }),
  useState: () => [null, hooks.state],
}));
import { useTrackSync } from './useTrackSync';

class FakeAudio extends EventTarget {
  static instances: FakeAudio[] = [];
  readyState = 4;
  currentTime = 0;
  playbackRate = 1;
  paused = true;
  src = '';
  preload = '';
  crossOrigin = '';
  play = vi.fn(async () => { this.paused = false; });
  pause = vi.fn(() => { this.paused = true; });
  load = vi.fn();
  removeAttribute = vi.fn();
  constructor() { super(); FakeAudio.instances.push(this); }
}
class FakeContext extends EventTarget {
  static instances: FakeContext[] = [];
  state = 'suspended';
  destination = {};
  gains: { gain: { value: number }; connect: ReturnType<typeof vi.fn>; disconnect: ReturnType<typeof vi.fn> }[] = [];
  resume = vi.fn(() => new Promise<void>(() => {}));
  close = vi.fn(async () => {});
  createGain() {
    const node = { gain: { value: 1 }, connect: vi.fn(), disconnect: vi.fn() };
    this.gains.push(node);
    return node;
  }
  createMediaElementSource() { return { connect: vi.fn(), disconnect: vi.fn() }; }
  constructor() { super(); FakeContext.instances.push(this); }
}
const flush = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
let cleanup: (() => void) | undefined;
function mount() {
  const video = Object.assign(new EventTarget(), {
    paused: false, seeking: false, muted: false, currentTime: 7, playbackRate: 1,
  });
  const result = useTrackSync({ current: video as unknown as HTMLVideoElement }, ['track.m4a'], [1.75]);
  cleanup = hooks.effects[0]() as () => void;
  return { video, result, context: FakeContext.instances[0], audio: FakeAudio.instances[0] };
}
beforeEach(() => {
  hooks.effects = [];
  hooks.state.mockReset();
  FakeContext.instances = [];
  FakeAudio.instances = [];
  vi.stubGlobal('Audio', FakeAudio);
  vi.stubGlobal('AudioContext', FakeContext);
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 1));
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
});
afterEach(() => { cleanup?.(); cleanup = undefined; vi.unstubAllGlobals(); });

describe('useTrackSync output readiness', () => {
  it('keeps mixed video audible while resume stays pending', async () => {
    const { result, video, context, audio } = mount();
    await flush();
    expect(result.active).toBe(false);
    expect(context.resume).toHaveBeenCalled();
    expect(video.muted).toBe(false);
    expect(context.gains[0].gain.value).toBe(0);
    expect(audio.play).not.toHaveBeenCalled();
    expect(hooks.state).not.toHaveBeenCalledWith('track.m4a');
  });
  it('switches to boosted tracks only after context running and source playback', async () => {
    const { video, context, audio } = mount();
    context.state = 'running';
    context.dispatchEvent(new Event('statechange'));
    expect(video.muted).toBe(false);
    await flush();
    expect(audio.play).toHaveBeenCalled();
    expect(audio.currentTime).toBe(7);
    expect(video.muted).toBe(true);
    expect(context.gains[0].gain.value).toBe(1);
    expect(context.gains[1].gain.value).toBe(1.75);
    expect(hooks.state).toHaveBeenLastCalledWith('track.m4a');
  });
  it('waits for buffered media as well as a running context', async () => {
    const { video, context, audio } = mount();
    audio.readyState = 1;
    context.state = 'running';
    context.dispatchEvent(new Event('statechange'));
    await flush();
    expect(video.muted).toBe(false);
    audio.readyState = 4;
    audio.dispatchEvent(new Event('canplay'));
    await flush();
    expect(video.muted).toBe(true);
  });
  it('silences separate tracks and restores mixed-mode state on suspension', async () => {
    const { context, audio } = mount();
    context.state = 'running';
    context.dispatchEvent(new Event('statechange'));
    await flush();
    context.state = 'suspended';
    context.dispatchEvent(new Event('statechange'));
    expect(context.gains[0].gain.value).toBe(0);
    expect(audio.paused).toBe(true);
    expect(hooks.state).toHaveBeenLastCalledWith(null);
  });
  it('does not switch output after cleanup with play still pending', async () => {
    const { video, context, audio } = mount();
    let finish!: () => void;
    audio.play.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
    context.state = 'running';
    context.dispatchEvent(new Event('statechange'));
    cleanup?.(); cleanup = undefined;
    finish();
    await flush();
    expect(video.muted).toBe(false);
    expect(context.gains[0].gain.value).toBe(0);
    expect(context.close).toHaveBeenCalled();
  });
});
