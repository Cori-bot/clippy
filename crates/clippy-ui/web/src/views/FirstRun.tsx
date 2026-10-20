import { useEffect, useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Select } from '@openai/apps-sdk-ui/components/Select';
import { Switch } from '@openai/apps-sdk-ui/components/Switch';
import { call, onDaemonEvent } from '../lib/ipc';
import { app, useApp } from '../lib/app';
import { clipHotkeyProblem, comboFromEvent } from '../lib/settings';
import type { Monitor } from '../lib/types';
import { KeycapInput } from '../components/ui/KeycapInput';
import { Alert, Check } from '../components/icons';

const EASE = 'ease-[cubic-bezier(0.22,1,0.36,1)]';

function Hint({ children }: { children: React.ReactNode }) {
  return <p className="my-0.5 max-w-[62ch] text-xs leading-relaxed text-secondary">{children}</p>;
}

export function FirstRun() {
  const a = useApp();
  const [step, setStep] = useState(1);
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const [monitorIndex, setMonitorIndex] = useState(0);
  // What the wizard sets up besides the capture itself. Both start on because
  // that is what makes the hotkey work without opening the app first, and both
  // are shown as choices so nothing is switched on out of sight.
  const [captureMode, setCaptureMode] = useState('desktop');
  const [autostart, setAutostart] = useState(true);
  const [autoArm, setAutoArm] = useState(true);
  // The whole config record: `clipHotkeyProblem` reads `hotkey_mode` as well as
  // `clip_hotkey`, and the row saves through `config.set` like a settings row.
  const [config, setConfig] = useState<Record<string, unknown>>({});
  /** The combination being recorded, or null while the row shows the saved one. */
  const [capture, setCapture] = useState<string | null>(null);
  const [heard, setHeard] = useState(false);
  const [clipDir, setClipDir] = useState('');

  const hotkey = String(config['clip_hotkey'] ?? 'alt+f10');

  // Why the clip hotkey is dead, or null while it works or nobody knows yet.
  // The daemon puts the answer on `status` as `clip_hotkey_bound`.
  const problem = clipHotkeyProblem(a.status?.clip_hotkey_bound, config);

  useEffect(() => {
    void (async () => {
      try {
        const list = await call<{ monitors: Monitor[] }>('monitors.list');
        setMonitors(list.monitors);
        const next = await call<Record<string, unknown>>('config.get');
        setConfig(next);
        setClipDir(String(next['clip_dir_resolved'] ?? ''));
      } catch (e) {
        app.toast('error', String(e));
      }
    })();
  }, []);

  // `onDaemonEvent` lives on Tauri's event bus, not on the component, so the
  // handler needs teardown (the unlisten is a Promise).
  useEffect(() => {
    const unlisten = onDaemonEvent((event) => {
      if (event.event === 'hotkey_pressed') setHeard(true);
      if (event.event === 'hotkey_rebound') {
        // Raised by this page's own `config.set`. Whether the new combination
        // actually bound reaches a client only through `status`, so re-read it.
        // No toast beside it: the strip below is already the answer.
        void app.refreshStatus();
      }
      if (event.event === 'config_changed') {
        // The picker is the daemon's dialog, so the chosen folder arrives here.
        setConfig((c) => ({ ...c, ...event.data }));
        const resolved = event.data['clip_dir_resolved'];
        if (typeof resolved === 'string') setClipDir(resolved);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  function captureHotkey(e: React.KeyboardEvent) {
    // Unconditional, before the combination is worked out: a bare Alt press
    // still reaches this handler, and letting that one through would move
    // focus to the window menu mid-recording.
    e.preventDefault();
    const combo = comboFromEvent(e.nativeEvent);
    if (combo !== null) setCapture(combo);
  }

  async function saveHotkey() {
    if (capture === null) return;
    try {
      // Saved here rather than carried to `finish`: the only way to find out
      // whether Windows will hand Clippy a combination is to have the daemon
      // try to register one, and `config.set` is what makes it try. The cost is
      // deliberate -- first run *is* the absence of config.toml, so writing one
      // here ends it.
      const result = await call<{ accepted: Record<string, unknown> }>('config.set', { clip_hotkey: capture });
      // Read back from `accepted`, not from `capture`.
      setConfig((c) => ({ ...c, ...result.accepted }));
      setCapture(null);
      // A press of the key they just replaced proves nothing about the new one.
      setHeard(false);
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  async function pickFolder() {
    try {
      // Answers as soon as the dialog is open; the result arrives as `config_changed`.
      await call('folder.pick');
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  async function finish() {
    try {
      // One call, because config.set is all-or-nothing: three keys in three
      // calls could leave a half-configured install behind.
      await call('config.set', {
        capture_mode: captureMode,
        monitor_index: monitorIndex,
        clip_hotkey: hotkey,
        auto_arm: autoArm,
        autostart,
      });
    } catch (e) {
      // Setup itself did not happen -- staying on this step, whose only
      // control retries the very call that just failed, is the correct response.
      app.toast('error', String(e));
      return;
    }

    // config.set succeeded, so setup is done: leave the wizard no matter what
    // happens next. Arming is a separate, retryable action the rail exposes.
    try {
      await call('arm');
    } catch (e) {
      app.toast('error', String(e));
    }
    await app.refreshStatus();
    app.view = 'grid';
  }

  const stateClass =
    problem !== null
      ? 'border-[var(--color-border-danger-outline)] text-[var(--color-text-danger)]'
      : heard
        ? 'border-[var(--color-border-success-outline)] text-[var(--color-text-success)]'
        : 'border-subtle text-secondary';

  return (
    <div className="flex-1 overflow-auto">
      <div className="mx-auto grid w-[min(560px,100%)] justify-items-start gap-3 px-6 py-[8vh]">
        <h1 className="heading-lg m-0">Configurer Clippy</h1>
        <p className="m-0 text-xs text-tertiary tabular-nums">Étape {step} sur 3</p>

        {step === 1 && (
          <div key="s1" className="grid w-full animate-[fade-in_250ms_ease-out] justify-items-start gap-3">
            <h2 className="heading-sm mt-3 mb-0">Que faut-il enregistrer&nbsp;?</h2>
            <div role="group" aria-label="Quoi enregistrer">
              <Select
                block={false}
                value={captureMode}
                options={[
                  { value: 'desktop', label: 'Jeux et bureau' },
                  { value: 'game', label: 'Jeux uniquement' },
                ]}
                onChange={(o) => setCaptureMode(o.value)}
              />
            </div>
            <Hint>
              {captureMode === 'game'
                ? 'Le replay attend qu’un jeu soit au premier plan en plein écran et n’enregistre jamais le bureau.'
                : 'Tout ce qui est affiché à l’écran est enregistré tant que le replay est actif.'}
            </Hint>
            {captureMode === 'desktop' && (
              <>
                <h2 className="heading-sm mt-3 mb-0">Quel écran capturer&nbsp;?</h2>
                <div role="group" aria-label="Écran">
                  <Select
                    block={false}
                    value={String(monitorIndex)}
                    options={monitors.map((m) => ({
                      value: String(m.index),
                      label: `${m.name} – ${m.width}×${m.height} (${m.adapter})`,
                    }))}
                    onChange={(o) => setMonitorIndex(Number(o.value))}
                  />
                </div>
              </>
            )}
            <div className="mt-2">
              <Button color="primary" onClick={() => setStep(2)}>Suivant</Button>
            </div>
          </div>
        )}

        {step === 2 && (
          <div key="s2" className="grid w-full animate-[fade-in_250ms_ease-out] justify-items-start gap-3">
            <h2 className="heading-sm mt-3 mb-0">Confirmer le raccourci de clip</h2>
            <Hint>
              Appuyer dessus maintenant. Certains overlays interceptent discrètement un raccourci dans les jeux&nbsp;: cette vérification confirme que Clippy le reçoit bien.
            </Hint>
            <div className="flex items-center gap-2">
              <KeycapInput
                combo={capture ?? hotkey}
                capturing={capture !== null}
                label="Raccourci de clip"
                oncapture={captureHotkey}
                oncancel={() => setCapture(null)}
                onstart={() => setCapture(hotkey)}
              />
              {capture !== null && capture !== hotkey && (
                <Button color="primary" size="sm" onClick={() => void saveHotkey()}>Enregistrer</Button>
              )}
            </div>
            {/* A status strip rather than a grey caption: it is the entire
                answer to the question the step asks, and on the machines where
                it matters it is bad news. `aria-live`, because all three states
                arrive after the page has painted. */}
            <p
              aria-live="polite"
              className={`m-0 flex w-full items-center gap-2.5 rounded-xl border bg-surface-secondary px-3.5 py-2.5 text-sm leading-snug transition-colors duration-200 ${EASE} ${stateClass}`}
            >
              {problem !== null ? (
                <Alert className="shrink-0" />
              ) : heard ? (
                <Check className="shrink-0" />
              ) : (
                <span className="size-2 shrink-0 animate-pulse rounded-full bg-[var(--color-text-tertiary)]" />
              )}
              <span>
                {problem ?? (heard ? 'Clippy l’a bien reçu.' : 'En attente d’un appui. Cliquer sur les touches ci-dessus pour choisir une autre combinaison.')}
              </span>
            </p>
            <div className="mt-1 flex items-center gap-2">
              <Button color="secondary" variant="ghost" onClick={() => setStep(1)}>Retour</Button>
              <Button color="primary" onClick={() => setStep(3)}>Suivant</Button>
            </div>
          </div>
        )}

        {step === 3 && (
          <div key="s3" className="grid w-full animate-[fade-in_250ms_ease-out] justify-items-start gap-3">
            <h2 className="heading-sm mt-3 mb-0">Où enregistrer les clips&nbsp;?</h2>
            {/* Read-only because the daemon owns the picker, and a typed path
                that does not exist is a refusal the user has to decode. */}
            <div className="flex w-full items-center gap-2">
              <span className="min-w-0 flex-1 truncate rounded-lg border border-default bg-surface px-2.5 py-2 text-xs text-secondary" title={clipDir}>
                {clipDir}
              </span>
              <Button color="secondary" variant="outline" onClick={() => void pickFolder()}>Choisir…</Button>
            </div>
            <Hint>Modifiable à tout moment dans Paramètres ou depuis l’icône Clippy de la zone de notification.</Hint>
            <Switch checked={autostart} label="Lancer Clippy avec Windows" onCheckedChange={setAutostart} />
            <Switch checked={autoArm} label="Activer le replay à chaque lancement" onCheckedChange={setAutoArm} />
            <Hint>
              Ainsi le raccourci fonctionne sans ouvrir l’application. Modifiable dans Paramètres ou depuis l’icône de la zone de notification.
            </Hint>
            <div className="mt-1 flex items-center gap-2">
              <Button color="secondary" variant="ghost" onClick={() => setStep(2)}>Retour</Button>
              <Button color="primary" onClick={() => void finish()}>Terminer et activer le replay</Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
