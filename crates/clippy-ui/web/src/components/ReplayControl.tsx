import { useEffect, useRef, useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { app, useApp } from '../lib/app';
import { call } from '../lib/ipc';
import { formatCombo } from '../lib/ui';
import { Check, Clips } from './icons';

/** Capture actions stay available while browsing or editing the library. */
export function ReplayControl() {
  const a = useApp();
  const [pending, setPending] = useState(false);
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    if (!saved) return;
    const timer = setTimeout(() => setSaved(false), 2000);
    return () => clearTimeout(timer);
  }, [saved]);
  const locked = useRef(false);
  const act = async (save: boolean) => {
    if (locked.current) return;
    locked.current = true;
    setPending(true);
    setSaved(false);
    try {
      if (save) {
        const result = await call<{ id?: string }>('clip');
        setSaved(typeof result.id === 'string');
      }
      else await app.toggleArm();
    } catch (error) {
      app.toast('error', String(error));
    } finally {
      locked.current = false;
      setPending(false);
    }
  };
  const ready = a.armed && a.ringUsed > 0;
  return (
    <section className="mt-auto border-t border-subtle pt-4" aria-label="Replay">
      <div className="mb-1 flex items-center gap-2 px-2 text-sm">
        <span className={`size-1.5 rounded-full ${a.armed ? 'bg-[var(--color-text-success)]' : 'bg-[var(--color-text-tertiary)]'}`} aria-hidden="true" />
        <span>{a.armed ? 'Replay actif' : a.waiting ? 'En attente d’un jeu' : 'Replay désactivé'}</span>
      </div>
      <p className="mb-2 px-2 text-xs leading-relaxed text-secondary">
        {a.armed ? `${Math.floor(a.ringUsed)} s · ${formatCombo(a.hotkey).join(' + ')}`
          : a.waiting ? 'La capture démarre au premier plan d’un jeu.' : 'Active le replay avant de jouer.'}
      </p>
      <Button block color="secondary" variant="soft" size="sm" className="motion-control"
        aria-label={a.armed ? 'Sauvegarder un clip' : a.waiting ? 'Désactiver le replay' : 'Activer le replay'}
        disabled={pending || !a.connected || (a.armed && !ready)}
        onClick={() => void act(a.armed)}>
        {saved && a.armed ? <Check className="save-confirmation" /> : <span className="motion-icon motion-nudge"><Clips /></span>}
        {pending ? 'Un instant…' : saved && a.armed ? 'Clip sauvegardé' : a.armed ? 'Sauvegarder' : a.waiting ? 'Désactiver le replay' : 'Activer le replay'}
      </Button>
      {a.armed && <Button block color="secondary" variant="ghost" size="xs" disabled={pending}
        onClick={() => void act(false)}>Désactiver le replay</Button>}
    </section>
  );
}
