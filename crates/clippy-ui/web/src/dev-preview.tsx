/** Dev-only visual fixture. Vite builds index.html only; no real files or daemon. */
import { createRoot } from 'react-dom/client';
import './index.css';
import type { ExportJobSnapshot, ExportStartArgs } from './lib/export-jobs';

const params = new URLSearchParams(location.search);
const mediaPreview = params.has('media');
const config = { config_file_exists: !params.has('setup'), check_for_updates: false, clip_hotkey: 'alt+f10', clip_dir_resolved: 'D:\\Clips', hotkey_mode: 'register' };
const status = { armed: true, encoder: 'H.264', monitor_index: 0, ring_seconds_used: 24, ring_seconds_total: 30, version: 'Aperçu', clip_dir: 'D:\\Clips', clip_hotkey_bound: !params.has('conflict') };
const titles = ['Dernière manche', 'Un virage parfait', 'Victoire en équipe', 'Le dernier duel', 'Nouvelle découverte', 'Au bon moment'];
const games = ['Counter-Strike 2', 'Forza Horizon 5', 'Valorant'];
const clips = Array.from({length: 24}, (_, i) => ({ id: `demo-${i}`, title: `${titles[i % 6]}${i > 5 ? ` ${i + 1}` : ''}`, game: games[i % 3], created: new Date(Date.UTC(2026, 9, 26 - i, 16)).toISOString(), duration_ms: mediaPreview ? 3000 : 15000 + i * 1000, bytes: 12000000 + i * 1000000, width: mediaPreview ? 160 : 1920, height: mediaPreview ? 96 : 1080, fps: mediaPreview ? 10 : 60, encoder: 'H.264', has_audio: true, favorite: i % 4 === 0, audio_tracks: mediaPreview ? ['Mix', 'Jeu synthétique', 'Micro synthétique'] : [] }));
let loadFailed = false;
let nextCallback = 0;
const calls: string[] = [];
let currentJob: (ExportJobSnapshot & {request_token?: string}) | null = null;
let jobStartedAt = 0;
let jobArgs: ExportStartArgs | null = null;
function refreshJob() {
  if (!currentJob || currentJob.state !== 'running') return currentJob;
  const elapsed = Date.now() - jobStartedAt;
  const percent = Math.min(100, Math.floor(elapsed / (params.has('slowExport') ? 300 : 70)));
  currentJob = percent < 100 ? {...currentJob, percent, stage: percent < 40 ? 'decoding' : 'writing'}
    : params.has('exportError') ? {...currentJob, percent, state: 'failed', error: 'Échec de test simulé.'}
    : {...currentJob, percent, state: 'completed', stage: 'finalizing', clip: {...clips[0], id: 'demo-export', audio_tracks: jobArgs?.output === 'share' ? ['Mix'] : clips[0].audio_tracks}};
  return currentJob;
}
const internals = {
  metadata: { currentWindow: { label: 'main' } },
  transformCallback: () => ++nextCallback,
  unregisterCallback: () => {},
  convertFileSrc: (path: string) => mediaPreview && /\.(mp4|m4a)$/.test(path)
    ? `http://127.0.0.1:1422/${path.endsWith('.mp4') ? 'editor-preview.mp4' : path.split(/[\\/]/).pop()}`
    : `data:image/svg+xml,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="640" height="400"><rect width="640" height="400" fill="#202124"/><text x="320" y="210" fill="#a6a7ab" font-family="sans-serif" font-size="16" text-anchor="middle">Miniature de vérification</text></svg>')}`,
  invoke: async (command: string, payload?: { cmd?: string; args?: Record<string, unknown> }) => {
    const cmd = payload?.cmd ?? command;
    calls.push(cmd);
    document.body.dataset.calls = JSON.stringify(calls);
    if (cmd === 'daemon_connected') return true;
    if (cmd === 'status') return {...status};
    if (cmd === 'config.get') {
      return {...config};
    }
    if (cmd === 'monitors.list') {
      if (params.has('loadError') && !loadFailed) { loadFailed = true; throw new Error('Chargement simulé indisponible'); }
      return { monitors: [{index: 0, name: 'Écran principal', width: 1920, height: 1080, adapter: 'Aperçu'}] };
    }
    if (cmd === 'config.set') {
      await new Promise(resolve => setTimeout(resolve, 250));
      if (params.has('saveError')) throw new Error('Sauvegarde simulée refusée');
      Object.assign(config, payload?.args); return {accepted: payload?.args};
    }
    if (cmd === 'arm') { status.armed = true; return {...status}; }
    if (cmd === 'disarm') { status.armed = false; return {}; }
    if (cmd === 'clip') return {id: 'saved-demo'};
    if (cmd === 'library.list') return { clips, total: clips.length, offset: 0, snapshot: 'demo', total_bytes: clips.reduce((sum,c) => sum+c.bytes,0) };
    if (cmd === 'library.copy' || cmd === 'library.reveal') return {};
    if (cmd === 'library.keyframes') return {keyframes: []};
    if (cmd === 'library.frames') {
      const clip = clips.find(item => item.id === payload?.args?.clip_id) ?? clips[0];
      return {timestamps_100ns: Array.from({length: Math.ceil(clip.duration_ms * clip.fps / 1000)}, (_, i) => Math.round(i * 10000000 / clip.fps)), end_100ns: clip.duration_ms * 10000};
    }
    if (cmd === 'library.tracks') return {tracks: [{index: 1, name: 'Jeu synthétique', file: 'editor-preview.t1.m4a'}, {index: 2, name: 'Micro synthétique', file: 'editor-preview.t2.m4a'}]};
    if (cmd === 'library.export') {
      document.body.dataset.lastExport = JSON.stringify(payload?.args);
      return {...clips[0], id: 'demo-export', audio_tracks: payload?.args?.output === 'share' ? ['Mix'] : clips[0].audio_tracks};
    }
    if (cmd === 'library.export.start') {
      if (currentJob && typeof payload?.args?.request_token === 'string' && payload.args.request_token === currentJob.request_token) return refreshJob();
      if (currentJob && ['queued', 'running', 'cancelling'].includes(currentJob.state)) throw new Error('Un export est déjà en cours.');
      jobArgs = payload?.args as ExportStartArgs;
      document.body.dataset.lastExport = JSON.stringify(jobArgs);
      jobStartedAt = Date.now();
      currentJob = {job_id: `demo-export-${jobStartedAt}`, state: 'running', percent: 0, stage: 'indexing',
        ...(typeof payload?.args?.request_token === 'string' ? {request_token: payload.args.request_token} : {})};
      return currentJob;
    }
    if (cmd === 'library.export.status') {
      if (typeof payload?.args?.request_token === 'string' && payload.args.request_token !== currentJob?.request_token) return null;
      return refreshJob();
    }
    if (cmd === 'library.export.cancel') {
      if (!currentJob || payload?.args?.job_id !== currentJob.job_id) throw new Error('Export introuvable.');
      currentJob = {...currentJob, state: 'cancelled'};
      return currentJob;
    }
    if (cmd === 'games.list') return {games: []};
    if (cmd === 'plugin:event|listen') return ++nextCallback;
    if (cmd === 'plugin:window|is_maximized') return false;
    return {};
  },
};
Object.assign(window, { __TAURI_INTERNALS__: internals, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} } });
const { App } = await import('./App');
createRoot(document.getElementById('root')!).render(<><App /><div className="pointer-events-none fixed bottom-1 left-1 z-50 text-[9px] text-tertiary">Aperçu · données fictives</div></>);
