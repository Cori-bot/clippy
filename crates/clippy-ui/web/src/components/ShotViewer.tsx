import { useEffect, useRef } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { app, useApp } from '../lib/app';
import { formatShotDate, shotUrl } from '../lib/shots';

/**
 * Full-window viewer under the 40 px title bar. `fixed`, not `absolute`: the
 * page inside the scrolling pane can grow past the window, and the control bar
 * would land below the fold.
 */
export function ShotViewer({ onClose }: { onClose: () => void }) {
  const a = useApp();
  const shot = a.shots[a.shotSelected] ?? null;
  const dir = a.status?.clip_dir ?? '';
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // The viewer opens over the page without being navigated to, so focus would
  // stay on the grid behind it. Land it on "Fermer".
  useEffect(() => {
    closeRef.current?.focus();
  }, [shot !== null]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.preventDefault();
        onCloseRef.current();
        return;
      }
      // Arrows walk the set without returning to the grid.
      const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
      if (step === 0) return;
      e.preventDefault();
      const next = app.shotSelected + step;
      if (next >= 0 && next < app.shots.length) app.shotSelected = next;
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  if (!shot) return null;
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Capture d’écran"
      className="fixed inset-x-0 bottom-0 top-10 z-30 flex flex-col bg-surface data-[open]:animate-[fade-in_200ms_ease-out]"
      data-open>
      <img
        src={dir ? shotUrl(dir, shot.id) : ''}
        alt={`Capture d’écran du ${formatShotDate(shot.created)}`}
        className="min-h-0 flex-1 bg-black object-contain"
      />
      <div className="flex items-center gap-2 border-t border-subtle px-4 py-2.5">
        <span className="text-sm text-secondary tabular-nums">
          {a.shotSelected + 1} / {a.shots.length}
        </span>
        <span className="ml-auto" />
        <Button color="secondary" variant="outline" size="sm" onClick={() => app.copyShot(shot.id)}>
          Copier
        </Button>
        <Button color="secondary" variant="outline" size="sm" onClick={() => app.revealShot(shot.id)}>
          Afficher dans le dossier
        </Button>
        <Button ref={closeRef} color="primary" size="sm" onClick={onClose}>
          Fermer
        </Button>
      </div>
    </div>
  );
}
