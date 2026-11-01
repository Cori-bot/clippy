import { describe, expect, it } from 'vitest';
import { MAX_WAVEFORM_BYTES, readBoundedAudio, waveformPeaks } from './media-previews';

describe('real audio preview peaks', () => {
  it('preserves brief transients and both stereo channels without inventing amplitude', () => {
    expect(waveformPeaks([new Float32Array([0, -0.8, 0, 0]), new Float32Array([0, 0, 0.4, 0])], 2))
      .toEqual([Math.fround(0.8), Math.fround(0.4)]);
  });
  it('represents silence as silence and handles short or damaged buffers', () => {
    expect(waveformPeaks([new Float32Array([0, 0])], 640)).toEqual([0, 0]);
    expect(waveformPeaks([new Float32Array([NaN, 5, Infinity])], 1)).toEqual([1]);
    expect(waveformPeaks([], 640)).toEqual([]);
  });
});

describe('bounded audio preview download', () => {
  it('rejects oversized advertised payloads before reading', async () => {
    await expect(readBoundedAudio(new Response('x', { headers: { 'content-length': String(MAX_WAVEFORM_BYTES + 1) } }), new AbortController().signal))
      .rejects.toThrow('volumineuse');
  });
  it('also enforces the limit when a server omits its content length', async () => {
    await expect(readBoundedAudio(new Response(new Uint8Array(MAX_WAVEFORM_BYTES + 1)), new AbortController().signal))
      .rejects.toThrow('volumineuse');
  });
  it('preserves bytes and aborts cancelled requests', async () => {
    expect(new Uint8Array(await readBoundedAudio(new Response(new Uint8Array([1, 2, 3])), new AbortController().signal))).toEqual(new Uint8Array([1, 2, 3]));
    const abort = new AbortController();
    abort.abort();
    await expect(readBoundedAudio(new Response('x'), abort.signal)).rejects.toThrow();
  });
});
