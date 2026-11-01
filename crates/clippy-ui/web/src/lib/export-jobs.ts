import { call } from './ipc';
import type { ClipMeta, ExportMode, ExportOutput } from './types';

export type ExportJobState = 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled';
export type ExportJobSnapshot = {
  job_id: string;
  request_token?: string;
  state: ExportJobState;
  percent: number;
  stage: string;
  clip?: ClipMeta;
  error?: string;
};
export type ExportStartArgs = {
  clip_id: string;
  start_ms: number;
  end_ms: number;
  mode: ExportMode;
  output: ExportOutput;
  tracks?: { index: number; gain: number }[];
  /** Precise share only: the daemon fits the file under this many megabytes. */
  size_limit_mb?: number;
  vertical_center?: number;
};
type Transport = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
const TERMINAL = new Set<ExportJobState>(['completed', 'failed', 'cancelled']);
const STATES = new Set<ExportJobState>(['queued', 'running', 'cancelling', ...TERMINAL]);

function snapshot(value: unknown): ExportJobSnapshot | null {
  if (value === null) return null;
  if (!value || typeof value !== 'object') throw new Error('Réponse du moteur d’export invalide.');
  const job = value as Partial<ExportJobSnapshot>;
  if (typeof job.job_id !== 'string' || !job.job_id || !STATES.has(job.state as ExportJobState)
    || typeof job.stage !== 'string' || !Number.isFinite(job.percent)
    || job.percent! < 0 || job.percent! > 100
    || (job.state === 'completed' && (!job.clip || typeof job.clip.id !== 'string'))) {
    throw new Error('Réponse du moteur d’export invalide.');
  }
  return job as ExportJobSnapshot;
}

/** A single daemon-owned job, observed independently of any mounted editor. */
export class ExportJobStore {
  job: ExportJobSnapshot | null = null;
  starting = false;
  recovering = false;
  cancelling = false;
  error: string | null = null;
  private connected = true;
  private epoch = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private completedId: string | null = null;
  private dismissedId: string | null = null;
  private pending: { token: string; recipe: string; error: string | null } | null = null;

  constructor(private request: Transport = call, private onComplete: () => void = () => {}) {}

  get active(): boolean {
    return this.starting || this.recovering || (this.job !== null && !TERMINAL.has(this.job.state));
  }

  async start(args: ExportStartArgs): Promise<void> {
    if (!this.connected) {
      this.error = 'Le moteur est déconnecté. Reconnecter Clippy avant d’exporter.';
      return;
    }
    if (this.active) return;
    const recipe = JSON.stringify(args);
    const token = this.pending?.recipe === recipe ? this.pending.token : crypto.randomUUID();
    this.pending = { token, recipe, error: null };
    this.stopTimer();
    const epoch = ++this.epoch;
    this.starting = true;
    this.job = null;
    this.error = null;
    this.dismissedId = null;
    try {
      const job = snapshot(await this.request('library.export.start', { ...args, request_token: token }));
      if (epoch !== this.epoch) return;
      if (!job) throw new Error('Le moteur n’a pas créé de tâche d’export.');
      if (job.request_token !== token) throw new Error('Le moteur a répondu pour une autre demande d’export.');
      this.apply(job);
      this.schedule(epoch);
    } catch (error) {
      if (epoch !== this.epoch) return;
      const startError = String(error);
      this.pending!.error = startError;
      // A lost response does not prove that the daemon refused the job.
      // Reconcile first, so a retry cannot duplicate a completed export.
      this.starting = false;
      await this.resume();
      if (!this.recovering && !this.job) this.error = startError;
    } finally {
      if (epoch === this.epoch) this.starting = false;
    }
  }

  /** Ask the daemon, rather than assuming a lost connection cancelled its work. */
  async resume(): Promise<void> {
    if (this.starting) return;
    this.connected = true;
    this.cancelling = false;
    this.stopTimer();
    const epoch = ++this.epoch;
    const wasActive = this.job !== null && !TERMINAL.has(this.job.state);
    this.recovering = true;
    try {
      const pending = this.pending;
      const job = snapshot(await this.request('library.export.status', pending ? { request_token: pending.token } : {}));
      if (epoch !== this.epoch) return;
      if (job && pending && job.request_token !== pending.token) throw new Error('Le moteur a répondu pour une autre demande d’export.');
      this.recovering = false;
      this.error = null;
      if (job && job.job_id === this.dismissedId && TERMINAL.has(job.state)) {
        this.job = null;
        this.error = null;
      } else if (job) {
        this.apply(job);
        this.schedule(epoch);
      } else {
        this.job = null;
        this.error = pending?.error ?? (wasActive ? 'L’export a été interrompu par le redémarrage du moteur.' : null);
      }
    } catch (error) {
      if (epoch !== this.epoch) return;
      this.error = `Impossible de retrouver l’export : ${String(error)}`;
      this.schedule(epoch, 1000);
    }
  }

  async cancel(): Promise<void> {
    const job = this.job;
    if (!job || TERMINAL.has(job.state) || this.cancelling || !this.connected) return;
    const epoch = this.epoch;
    this.cancelling = true;
    this.error = null;
    try {
      const result = snapshot(await this.request('library.export.cancel', { job_id: job.job_id }));
      if (epoch !== this.epoch || this.job?.job_id !== job.job_id) return;
      if (!result || result.job_id !== job.job_id) throw new Error('Réponse d’annulation invalide.');
      this.apply(result);
      this.schedule(epoch);
    } catch (error) {
      if (epoch === this.epoch) this.error = `Annulation non confirmée : ${String(error)}`;
    } finally {
      if (epoch === this.epoch) this.cancelling = false;
    }
  }

  disconnect(): void {
    this.connected = false;
    ++this.epoch;
    this.stopTimer();
    this.starting = false;
    this.cancelling = false;
    this.recovering = true;
  }

  dismiss(): void {
    if (this.active) return;
    this.dismissedId = this.job?.job_id ?? null;
    this.job = null;
    this.error = null;
    this.stopTimer();
  }

  private stopTimer(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
  }

  private schedule(epoch: number, delay = 350): void {
    this.stopTimer();
    if (!this.connected || !this.job || TERMINAL.has(this.job.state)) return;
    this.timer = setTimeout(() => void this.poll(epoch), delay);
  }

  private async poll(epoch: number): Promise<void> {
    this.timer = undefined;
    const id = this.job?.job_id;
    if (!id || epoch !== this.epoch || !this.connected) return;
    try {
      const job = snapshot(await this.request('library.export.status', { job_id: id }));
      if (epoch !== this.epoch || this.job?.job_id !== id) return;
      this.recovering = false;
      if (!job) {
        this.job = null;
        this.error = 'Le moteur ne retrouve plus cette tâche d’export.';
        return;
      }
      if (job.job_id !== id) throw new Error('Le moteur a répondu pour un autre export.');
      this.apply(job);
      this.schedule(epoch);
    } catch (error) {
      if (epoch !== this.epoch) return;
      this.error = `Suivi de l’export indisponible : ${String(error)}`;
      this.schedule(epoch, 1000);
    }
  }

  private apply(job: ExportJobSnapshot): void {
    const previous = this.job;
    // A status response started before cancellation/completion can arrive last.
    if (previous?.job_id === job.job_id) {
      if (TERMINAL.has(previous.state)) return;
      if (previous.state === 'cancelling' && (job.state === 'queued' || job.state === 'running')) {
        job = { ...job, state: 'cancelling' };
      }
      job = { ...job, percent: Math.max(previous.percent, job.percent) };
    }
    this.job = job;
    if (this.pending?.token === job.request_token) this.pending = null;
    this.error = job.state === 'failed' ? job.error ?? 'L’export a échoué.' : null;
    if (TERMINAL.has(job.state)) this.stopTimer();
    if (job.state === 'completed' && this.completedId !== job.job_id) {
      this.completedId = job.job_id;
      try { this.onComplete(); }
      catch (error) { this.error = `Export terminé, mais la bibliothèque n’a pas pu être actualisée : ${String(error)}`; }
    }
  }
}
