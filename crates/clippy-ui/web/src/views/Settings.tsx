import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Alert } from '@openai/apps-sdk-ui/components/Alert';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { call, onDaemonEvent } from '../lib/ipc';
import { app, useApp, useUpdates } from '../lib/app';
import {
  FIELDS,
  SECTIONS,
  SECTION_LABELS,
  hotkeyProblem,
  inactiveReason,
  unknownKeys,
  validate,
  type Field as FieldDef,
} from '../lib/settings';
import { Field, Row } from '../components/Field';
import { Rearm } from '../components/icons';
import type { Monitor } from '../lib/types';

const EASE = 'ease-[cubic-bezier(0.22,1,0.36,1)]';

export function Settings() {
  const a = useApp();
  const u = useUpdates();

  const [config, setConfig] = useState<Record<string, unknown>>({});
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const [extras, setExtras] = useState<string[]>([]);
  // `config` starts empty, and an empty config renders every toggle off and every
  // stepper at 0 -- values that are not the user's. The sections stay dimmed
  // until the daemon has actually answered.
  const [loaded, setLoaded] = useState(false);

  const [version, setVersion] = useState('');
  const [checking, setChecking] = useState(false);
  const [checked, setChecked] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      // Before the fields render, because a hotkey row's warning is read off
      // `status` and this page can be opened long after the connect that last
      // fetched one.
      await app.refreshStatus();
      const next = await call<Record<string, unknown>>('config.get');
      setConfig(next);
      setLoaded(true);
      setExtras(unknownKeys(next));
      const list = await call<{ monitors: Monitor[] }>('monitors.list');
      setMonitors(list.monitors);
    } catch (e) {
      app.toast('error', String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // This handler belongs to a page the user opens and leaves through the rail:
  // left registered, every visit would stack another handler writing to a dead
  // instance. The unlisten is a Promise, run in the cleanup.
  useEffect(() => {
    const unlisten = onDaemonEvent((event) => {
      if (event.event === 'hotkey_rebound') {
        if (event.data['registered'] === false) {
          app.toast('error', `Windows a refusé d’attribuer ${event.data['spec']} à Clippy. Une autre application l’utilise déjà.`);
        }
        // The toast is the alert; `status` is the record. Re-read it either way,
        // because a rebind that *succeeded* is what clears a warning left over
        // from the combination before it.
        void app.refreshStatus();
      }
      if (event.event === 'config_changed') {
        // The sound dialog and the tray change config behind this page's back.
        setConfig((c) => ({ ...c, ...event.data }));
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    invoke<string>('update_current_version').then(setVersion, () => {});
  }, []);

  // `set` is read by memoised children through a stable identity.
  const armedRef = useRef(a.armed);
  armedRef.current = a.armed;

  async function set(key: string, value: unknown) {
    const complaint = validate(key, value);
    if (complaint) {
      app.toast('error', complaint);
      return;
    }
    try {
      const result = await call<{ accepted: Record<string, unknown>; requires_rearm: string[] }>('config.set', { [key]: value });
      // Read back from `accepted`, which the daemon takes out of the saved
      // config rather than echoing the request: an accepted value that differs
      // from what was typed is exactly what the field should now show.
      setConfig((c) => ({ ...c, ...result.accepted }));
      if (result.requires_rearm.length > 0 && armedRef.current) {
        app.addRearmNeeded(result.requires_rearm);
      }
    } catch (e) {
      app.toast('error', String(e));
      // The daemon refused, so nothing was written: put the field back to the
      // truth rather than leaving the typed value on screen looking saved.
      await load();
    }
  }

  // Both pickers answer as soon as the dialog is open; the chosen folder or
  // file arrives as a `config_changed` event, handled above.
  async function run(method: string) {
    try {
      await call(method);
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  function jump(section: string) {
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    document.getElementById(`section-${section}`)?.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' });
  }

  /**
   * Unlike the check on launch, this one reports either way -- the user asked,
   * so silence would read as a broken button. Renders from `update_check`'s
   * return value, never from the `clippy-update` channel, which stays silent
   * while an install is in flight.
   */
  async function checkNow() {
    setChecking(true);
    setChecked(null);
    try {
      const found = await invoke<{ version: string } | null>('update_check');
      setChecked(found ? `Clippy ${found.version} est disponible.` : 'Clippy est à jour.');
    } catch (e) {
      // Rust already writes a complete, correctly-attributed sentence here;
      // render it unprefixed.
      setChecked(String(e));
    } finally {
      setChecking(false);
    }
  }

  const row = (field: FieldDef) => (
    <Field
      key={field.key}
      field={field}
      config={config}
      monitors={monitors}
      problem={hotkeyProblem(field, field.boundKey ? a.status?.[field.boundKey] : null, config)}
      inactive={inactiveReason(field, config)}
      onset={set}
      onpickfolder={() => void run('folder.pick')}
      onpicksound={() => void run('sound.pick')}
      ontestsound={() => void run('sound.test')}
    />
  );

  return (
    <div className="mx-auto max-w-[860px]">
      <h1 className="heading-lg mb-1">Paramètres</h1>
      <p className="mb-3 text-sm text-secondary">
        Les modifications s’appliquent au prochain clip. Certaines demandent de réactiver le replay, et Clippy indique lesquelles.
      </p>

      {/* Sticky at the top of the scrollport, the re-arm notice parked just
          under it: the page is long and the headings are its only landmarks.
          Opaque, so the page does not show through -- including the strip above
          it: sticky `top-0` sits below <main>'s 18 px padding, so the `before`
          covers that strip. */}
      <nav
        aria-label="Sections des paramètres"
        className="sticky top-0 z-[4] mb-3.5 flex h-9 flex-wrap items-center gap-1 border-b border-subtle bg-surface-tertiary before:absolute before:inset-x-0 before:-top-[18px] before:h-[18px] before:bg-surface-tertiary"
      >
        {SECTIONS.map((section) => (
          <button
            key={section}
            type="button"
            onClick={() => jump(section)}
            className={`cursor-pointer rounded-full border-0 bg-transparent px-2.5 py-1 text-xs text-secondary transition-[background-color,color,transform] duration-200 ${EASE} hover:bg-surface-secondary hover:text-default active:scale-[0.97]`}
          >
            {SECTION_LABELS[section]}
          </button>
        ))}
      </nav>

      {a.rearmNeeded.length > 0 && (
        // Sticky: the keys that need a re-arm are scattered down a long page,
        // so a notice at the top of the document would have to be scrolled
        // back to. Nothing between here and <main> sets transform/filter.
        <div className="sticky top-9 z-[3] mb-4 bg-surface-tertiary pb-1">
          <Alert
            color="success"
            variant="soft"
            indicator={<Rearm />}
            description={
              <>
                Réactiver le replay pour appliquer&nbsp;: {a.rearmNeeded.map((k) => FIELDS.find((f) => f.key === k)?.label ?? k).join(', ')}
              </>
            }
            actions={
              <Button color="primary" size="sm" onClick={() => void app.rearmNow()}>
                Réactiver maintenant
              </Button>
            }
            actionsPlacement="end"
          />
        </div>
      )}

      {SECTIONS.map((section) => {
        const fields = FIELDS.filter((f) => f.section === section);
        const advanced = fields.filter((f) => f.advanced);
        return (
          <section
            key={section}
            id={`section-${section}`}
            aria-busy={!loaded}
            className={`mb-6 scroll-mt-12 transition-opacity duration-200 ${EASE} ${loaded ? '' : 'opacity-50'}`}
          >
            <h2 className="mb-2 text-xs font-semibold tracking-wider text-secondary uppercase">{SECTION_LABELS[section]}</h2>
            <div className="rounded-2xl border border-subtle bg-surface-secondary px-4">
              {fields.filter((f) => !f.advanced).map(row)}
              {advanced.length > 0 && (
                <details className="border-t border-subtle">
                  <summary className={`cursor-pointer py-3 text-xs text-secondary transition-colors duration-200 ${EASE} hover:text-default`}>
                    Avancé
                  </summary>
                  {advanced.map(row)}
                </details>
              )}
              {section === 'Updates' && (
                <Row
                  title="Version"
                  left={
                    <>
                      <span>Version de Clippy actuellement utilisée.</span>
                      {checked && <span className="mt-1 block text-default">{checked}</span>}
                    </>
                  }
                >
                  <span className="text-sm text-secondary tabular-nums">{version}</span>
                  <Button color="secondary" variant="outline" size="sm" disabled={checking || u.busy} onClick={() => void checkNow()}>
                    {checking ? 'Vérification…' : 'Vérifier maintenant'}
                  </Button>
                </Row>
              )}
            </div>
          </section>
        );
      })}

      {extras.length > 0 && (
        <p className="text-xs text-secondary">
          Le service Clippy expose des paramètres que cette application n’affiche pas encore&nbsp;: {extras.join(', ')}. Ils se modifient dans config.toml.
        </p>
      )}
    </div>
  );
}
