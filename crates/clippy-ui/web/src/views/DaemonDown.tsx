import { Button } from '@openai/apps-sdk-ui/components/Button';
import { useState } from 'react';
import { startDaemon } from '../lib/ipc';
import { app } from '../lib/app';

export function DaemonDown() {
  const [starting, setStarting] = useState(false);

  async function start() {
    setStarting(true);
    // The supervisor reconnects by itself and removes this panel when it does;
    // if that has not happened by now the service is not coming, and a button
    // stuck on "Démarrage…" would be a dead end.
    setTimeout(() => {
      setStarting((still) => {
        if (still) app.toast('error', 'Le service Clippy ne répond pas. Réessayer.');
        return false;
      });
    }, 15000);
    try {
      await startDaemon();
    } catch (e) {
      app.toast('error', String(e));
      setStarting(false);
    }
  }

  return (
    <div className="grid flex-1 place-content-center justify-items-center gap-3 p-10 text-center">
      <h2 className="heading-lg">Clippy n’est pas lancé</h2>
      <p className="max-w-[42ch] text-sm text-secondary">
        Le service en arrière-plan gère le tampon de replay et le raccourci de clip. Rien n’est capturé pour le moment.
      </p>
      <Button color="primary" disabled={starting} onClick={() => void start()}>
        {starting ? 'Démarrage…' : 'Démarrer Clippy'}
      </Button>
    </div>
  );
}
