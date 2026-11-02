/**
 * Diagnostics mode, window half: a log of what is done in the window and how
 * the window behaves, appended to `%APPDATA%\clippy\logs\ui-YYYYMMDD.jsonl`
 * through the `diag_append` command. The daemon writes its own log and the
 * app + PC metrics beside it (clippy-daemon's `diag.rs`).
 *
 * Off unless the `diagnostics` setting is on. What is recorded: clicks, double
 * clicks and right clicks (which control, by its label), keys (shortcuts by
 * name; inside a text field only that a key was typed, never the text),
 * setting changes, views, toasts, daemon calls with their duration and
 * outcome, daemon events, errors and console warnings, long tasks, video
 * errors and stalls, and every 10 s the frame rate, worst frame, JS heap and
 * DOM size.
 */
import { invoke } from '@tauri-apps/api/core';

type Record_ = Record<string, unknown>;

let enabled = false;
let installed = false;
const buffer: string[] = [];
const MAX_BUFFER = 5000;

export function diagEnabled(): boolean {
  return enabled;
}

export function setDiagnostics(on: boolean): void {
  if (on === enabled) return;
  enabled = on;
  if (on) {
    install();
    diag('diagnostics', { on: true, agent: navigator.userAgent, screen: `${screen.width}x${screen.height}@${devicePixelRatio}`, window: `${innerWidth}x${innerHeight}` });
  }
}

/** Records one event. Cheap and a no-op while diagnostics is off. */
export function diag(kind: string, data: Record_ = {}): void {
  if (!enabled) return;
  if (buffer.length >= MAX_BUFFER) buffer.shift();
  try {
    buffer.push(JSON.stringify({ t: new Date().toISOString(), kind, ...data }));
  } catch {
    buffer.push(JSON.stringify({ t: new Date().toISOString(), kind, unserializable: true }));
  }
}

function localDay(): string {
  const d = new Date();
  return `${d.getFullYear()}${String(d.getMonth() + 1).padStart(2, '0')}${String(d.getDate()).padStart(2, '0')}`;
}

async function flush(): Promise<void> {
  if (buffer.length === 0) return;
  const lines = buffer.splice(0, buffer.length);
  try {
    await invoke('diag_append', { day: localDay(), lines });
  } catch {
    // Diagnostics never becomes the failure; the lines are dropped.
  }
}

/** Short, human-readable name of the control an event landed on. */
export function describe(target: EventTarget | null): Record_ {
  if (!(target instanceof Element)) return {};
  const control = target.closest('button, a, input, select, textarea, label, [role], [data-clip-id], video') ?? target;
  const text = (el: Element | null | undefined) => (el?.textContent ?? '').replace(/\s+/g, ' ').trim();
  const labelledBy = control.getAttribute('aria-labelledby');
  let label = control.getAttribute('aria-label') ?? control.getAttribute('title') ??
    (labelledBy ? text(document.getElementById(labelledBy)) : '') ?? '';
  if (!label) label = text(control);
  // A bare switch or slider: name it after the setting row it sits in.
  for (let up = control.parentElement, depth = 0; !label && up && depth < 5; up = up.parentElement, depth++) {
    label = text(up);
  }
  label = label.slice(0, 80);
  const out: Record_ = { tag: control.tagName.toLowerCase() };
  const role = control.getAttribute('role');
  if (role) out.role = role;
  if (label) out.label = label;
  if (control instanceof HTMLInputElement) out.type = control.type;
  return out;
}

function editable(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLInputElement && !['checkbox', 'radio', 'range', 'button', 'submit'].includes(target.type)));
}

function install(): void {
  if (installed) return;
  installed = true;
  setInterval(() => void flush(), 2000);
  addEventListener('beforeunload', () => void flush());

  for (const type of ['click', 'dblclick', 'contextmenu'] as const) {
    addEventListener(type, (e) => diag(type, { ...describe(e.target), x: Math.round(e.clientX), y: Math.round(e.clientY) }), true);
  }
  addEventListener('keydown', (e) => {
    if (editable(e.target) && !e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
      diag('type', describe(e.target));
      return;
    }
    const combo = [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Meta', e.key].filter(Boolean).join('+');
    diag('key', { combo, repeat: e.repeat, ...describe(e.target) });
  }, true);
  addEventListener('change', (e) => {
    const t = e.target;
    const value = t instanceof HTMLInputElement && (t.type === 'checkbox' || t.type === 'radio') ? t.checked
      : t instanceof HTMLInputElement && t.type === 'range' ? t.value
      : t instanceof HTMLSelectElement ? t.value
      : t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement ? `(${t.value.length} caractères)` : undefined;
    diag('change', { ...describe(t), value });
  }, true);
  addEventListener('error', (e: Event) => {
    if (e instanceof ErrorEvent) {
      diag('error', { message: e.message, source: e.filename, line: e.lineno, col: e.colno, stack: e.error instanceof Error ? e.error.stack : undefined });
    } else if (e.target instanceof HTMLMediaElement) {
      diag('media-error', { src: e.target.currentSrc.slice(-80), code: e.target.error?.code, message: e.target.error?.message });
    }
  }, true);
  for (const type of ['stalled', 'waiting'] as const) {
    addEventListener(type, (e) => {
      if (e.target instanceof HTMLMediaElement) diag(`media-${type}`, { src: e.target.currentSrc.slice(-80), at: e.target.currentTime });
    }, true);
  }
  document.addEventListener('securitypolicyviolation', (e) => diag('csp', { directive: e.effectiveDirective, blocked: e.blockedURI.slice(-80), source: e.sourceFile, line: e.lineNumber }));
  addEventListener('unhandledrejection', (e) => diag('rejection', { reason: String(e.reason), stack: e.reason instanceof Error ? e.reason.stack : undefined }));
  addEventListener('focus', () => diag('window-focus'));
  addEventListener('blur', () => diag('window-blur'));
  document.addEventListener('visibilitychange', () => diag('visibility', { state: document.visibilityState }));
  addEventListener('resize', debounce(() => diag('resize', { window: `${innerWidth}x${innerHeight}` }), 500));

  for (const level of ['error', 'warn'] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      diag(`console-${level}`, { message: args.map((a) => (a instanceof Error ? a.stack ?? a.message : typeof a === 'string' ? a : safeJson(a))).join(' ').slice(0, 2000) });
      original(...args);
    };
  }

  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) diag('long-task', { ms: Math.round(entry.duration) });
    }).observe({ type: 'longtask', buffered: false });
  } catch {
    // Not supported: the frame sampler below still catches jank.
  }

  // Frame pacing: average fps and the worst gap between frames, per 10 s.
  let frames = 0;
  let worst = 0;
  let last = performance.now();
  const tick = (now: number) => {
    frames++;
    worst = Math.max(worst, now - last);
    last = now;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  let windowStart = performance.now();
  setInterval(() => {
    const now = performance.now();
    const memory = (performance as unknown as { memory?: { usedJSHeapSize: number; totalJSHeapSize: number } }).memory;
    diag('perf', {
      fps: Math.round((frames / (now - windowStart)) * 1000),
      worst_frame_ms: Math.round(worst),
      heap_mb: memory ? Math.round(memory.usedJSHeapSize / 1048576) : undefined,
      dom_nodes: document.getElementsByTagName('*').length,
      visible: document.visibilityState,
    });
    frames = 0;
    worst = 0;
    windowStart = now;
  }, 10000);
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function debounce(fn: () => void, ms: number): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return () => {
    clearTimeout(timer);
    timer = setTimeout(fn, ms);
  };
}

/** Shortens a daemon call's arguments for the log: long strings trimmed. */
export function summarizeArgs(args: Record_): Record_ {
  const out: Record_ = {};
  for (const [key, value] of Object.entries(args)) {
    out[key] = typeof value === 'string' && value.length > 120 ? `${value.slice(0, 120)}…` : value;
  }
  return out;
}
