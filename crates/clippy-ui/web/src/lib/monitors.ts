import type { Monitor } from './types';

/** Upright when taller than wide: the desktop size already follows the rotation. */
export function isPortrait(m: Monitor): boolean {
  return m.height > m.width;
}

/** Windows puts the main screen's top-left corner at (0, 0). */
export function isPrimary(m: Monitor): boolean {
  return m.left === 0 && m.top === 0;
}

/** « Écran 2 · 1080×1920 · vertical · principal » */
export function monitorLabel(m: Monitor): string {
  const parts = [`Écran ${m.index + 1}`, `${m.width}×${m.height}`, isPortrait(m) ? 'vertical' : 'horizontal'];
  if (isPrimary(m)) parts.push('principal');
  return parts.join(' · ');
}

export type MonitorBox = { monitor: Monitor; x: number; y: number; w: number; h: number };

/**
 * The screens scaled into at most `maxW` x `maxH` pixels, keeping their
 * arrangement and proportions, with a 2 px gap so touching screens stay
 * apart. Returns the boxes and the size actually used.
 */
export function monitorLayout(monitors: Monitor[], maxW: number, maxH: number): { width: number; height: number; boxes: MonitorBox[] } {
  if (monitors.length === 0) return { width: 0, height: 0, boxes: [] };
  const minX = Math.min(...monitors.map((m) => m.left));
  const minY = Math.min(...monitors.map((m) => m.top));
  const maxX = Math.max(...monitors.map((m) => m.left + m.width));
  const maxY = Math.max(...monitors.map((m) => m.top + m.height));
  const scale = Math.min(maxW / Math.max(1, maxX - minX), maxH / Math.max(1, maxY - minY));
  const gap = 2;
  const boxes = monitors.map((monitor) => ({
    monitor,
    x: Math.round((monitor.left - minX) * scale) + gap / 2,
    y: Math.round((monitor.top - minY) * scale) + gap / 2,
    w: Math.max(8, Math.round(monitor.width * scale) - gap),
    h: Math.max(8, Math.round(monitor.height * scale) - gap),
  }));
  return { width: Math.round((maxX - minX) * scale), height: Math.round((maxY - minY) * scale), boxes };
}
