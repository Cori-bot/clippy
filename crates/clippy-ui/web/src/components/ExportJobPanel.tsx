import { Button } from '@openai/apps-sdk-ui/components/Button';
import { app, exportJobs, useExportJobs } from '../lib/app';
import { formatBytes } from '../lib/clips';

const STAGES: Record<string, string> = {
  queued: 'Préparation', indexing: 'Analyse de la vidéo', decoding: 'Lecture des pistes',
  mixing: 'Mixage audio', writing: 'Écriture du MP4', finalizing: 'Finalisation',
};

export function ExportJobPanel() {
  const jobs = useExportJobs();
  const job = jobs.job;
  if (!job && !jobs.starting && !jobs.recovering && !jobs.error) return null;
  const title = jobs.starting ? 'Préparation de l’export…'
    : jobs.recovering && !job ? 'Vérification de l’export…'
    : job?.state === 'completed' ? 'MP4 exporté'
    : job?.state === 'cancelled' ? 'Export annulé'
    : job?.state === 'failed' ? 'Export impossible'
    : job?.state === 'cancelling' ? 'Annulation demandée…' : 'Export vidéo';
  return (
    <section className="mt-4 shrink-0 border-t border-subtle px-2 pt-3" aria-label="Export vidéo">
      <p className="mb-1 text-sm" role="status">{title}</p>
      {jobs.active && job && (
        <>
          <p className="mb-2 flex justify-between gap-2 text-xs text-secondary">
            <span>{STAGES[job.stage] ?? 'Traitement en cours'}</span><span className="tabular-nums">{job.percent} %</span>
          </p>
          <progress className="export-progress mb-2 h-1 w-full" max={100} value={job.percent} aria-label="Avancement de l’export" />
          <Button size="xs" color="secondary" variant="ghost" disabled={jobs.cancelling || job.state === 'cancelling'}
            onClick={() => void exportJobs.cancel()}>Annuler l’export</Button>
        </>
      )}
      {job?.state === 'completed' && job.clip && (
        <>
          <p className="mb-2 text-xs text-secondary">{job.clip.width} × {job.clip.height} · {formatBytes(job.clip.bytes)}</p>
          <Button size="xs" color="primary" className="mb-2" onClick={() => void app.copyClip(job.clip!.id)}>Copier le fichier</Button>
        </>
      )}
      {(jobs.error || job?.error) && <p className="mb-2 break-words text-xs text-danger" role="alert">{jobs.error ?? job?.error}</p>}
      {jobs.error && <Button size="xs" color="secondary" variant="ghost" onClick={() => void exportJobs.resume()}>Réessayer le suivi</Button>}
      {!jobs.active && !jobs.starting && <Button size="xs" color="secondary" variant="ghost" onClick={() => exportJobs.dismiss()}>Fermer</Button>}
    </section>
  );
}
