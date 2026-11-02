import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { DaemonEvent } from './types';
import { diag, diagEnabled, summarizeArgs } from './diag';

/** One command to the daemon. Rejects with the daemon's own error text. */
export async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!diagEnabled()) return await invoke<T>('clippy_call', { cmd, args });
  const started = performance.now();
  try {
    const result = await invoke<T>('clippy_call', { cmd, args });
    diag('call', { cmd, args: summarizeArgs(args), ms: Math.round(performance.now() - started), ok: true });
    return result;
  } catch (e) {
    diag('call', { cmd, args: summarizeArgs(args), ms: Math.round(performance.now() - started), ok: false, error: String(e) });
    throw e;
  }
}

export async function startDaemon(): Promise<void> {
  await invoke('start_daemon');
}

/** Whether the supervisor is connected right now, asked rather than awaited. */
export async function daemonConnected(): Promise<boolean> {
  return await invoke<boolean>('daemon_connected');
}

export function onDaemonEvent(handler: (event: DaemonEvent) => void) {
  return listen<DaemonEvent>('clippy-event', (e) => handler(e.payload));
}

/** One diagnostics line per daemon event, from the app's single permanent listener. */
export function logDaemonEvent(event: DaemonEvent) {
  if (diagEnabled()) diag('event', { event: event.event, data: event.event === 'stats' ? event.data : summarizeArgs((event.data ?? {}) as Record<string, unknown>) });
}

export function onConnected(handler: (status: unknown) => void) {
  return listen('clippy-connected', (e) => handler(e.payload));
}

export function onDisconnected(handler: () => void) {
  return listen('clippy-disconnected', () => handler());
}
