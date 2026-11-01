import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Select } from '@openai/apps-sdk-ui/components/Select';
import { Slider } from '@openai/apps-sdk-ui/components/Slider';
import { Switch } from '@openai/apps-sdk-ui/components/Switch';
import { onDaemonEvent } from '../lib/ipc';
import {
  CUSTOM_TIER,
  comboFromEvent,
  hotkeySaveTarget,
  hotkeySeed,
  tierFor,
  tierOptions,
  type Field as FieldDef,
} from '../lib/settings';
import { resolveStepperInput, stepBy } from '../lib/ui';
import type { Monitor } from '../lib/types';
import { AppPicker } from './AppPicker';
import { KeycapInput } from './ui/KeycapInput';

const EASE = 'ease-[cubic-bezier(0.22,1,0.36,1)]';

/** The unit shown inside the stepper, so it is no longer only in the help text. */
const UNITS: Record<string, string> = {
  fps: 'fps',
  replay_seconds: 's',
  bitrate_kbps: 'kbps',
  max_bitrate_kbps: 'kbps',
  max_library_gb: 'Go',
  stats_seconds: 's',
};

/**
 * One settings row: label and help on the left, control on the right. Shared by
 * the generated rows and the hand-written Version row of the page.
 */
export function Row({
  title,
  dimmed = false,
  left,
  children,
}: {
  title: string;
  dimmed?: boolean;
  /** Everything under the title: help, details, reasons. */
  left?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-t border-subtle py-3.5 first:border-t-0">
      <div className={`flex min-w-0 flex-1 flex-col gap-0.5 transition-opacity duration-200 ${EASE} ${dimmed ? 'opacity-55' : ''}`}>
        <b className="text-sm font-medium text-default">{title}</b>
        <div className="max-w-[62ch] text-xs leading-relaxed text-secondary">{left}</div>
      </div>
      <div className={`flex max-w-[58%] flex-wrap items-center justify-end gap-2 transition-opacity duration-200 ${EASE} ${dimmed ? 'opacity-55' : ''}`}>
        {children}
      </div>
    </div>
  );
}

/** A number box with − / + buttons, the unit inside, and clamped bounds. */
export function Stepper({
  value,
  min,
  max,
  step = 1,
  unit,
  label,
  onchange,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  label: string;
  onchange: (v: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  // The box shows what actually took effect: re-sync whenever the value lands,
  // including a rejected edit that fell back to the old value.
  useEffect(() => setDraft(String(value)), [value]);

  function commit() {
    const resolved = resolveStepperInput(draft, value, min, max);
    setDraft(String(resolved));
    // An unresolved or unchanged edit doesn't reach the daemon at all.
    if (resolved !== value) onchange(resolved);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    const delta = e.key === 'ArrowUp' ? 1 : e.key === 'ArrowDown' ? -1 : 0;
    if (delta !== 0) {
      e.preventDefault();
      onchange(stepBy(value, delta, min, max, step));
    } else if (e.key === 'Home') {
      e.preventDefault();
      onchange(min);
    } else if (e.key === 'End') {
      e.preventDefault();
      onchange(max);
    } else if (e.key === 'Enter') {
      commit();
    }
  }

  return (
    <div className={`flex items-center rounded-lg border border-default bg-surface transition-[border-color,box-shadow] duration-200 ${EASE} has-[:focus-visible]:border-strong has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-[var(--color-border-strong)]/30`}>
      <Button
        color="secondary"
        variant="ghost"
        size="sm"
        uniform
        pill={false}
        aria-label={`${label} : diminuer`}
        disabled={value <= min}
        onClick={() => onchange(stepBy(value, -1, min, max, step))}
      >
        &minus;
      </Button>
      <input
        className="w-[58px] border-0 bg-transparent p-0 text-center text-sm text-default tabular-nums outline-none"
        type="text"
        inputMode="numeric"
        aria-label={label}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
      {unit && <span className="pr-2 text-xs text-tertiary">{unit}</span>}
      <Button
        color="secondary"
        variant="ghost"
        size="sm"
        uniform
        pill={false}
        aria-label={`${label} : augmenter`}
        disabled={value >= max}
        onClick={() => onchange(stepBy(value, 1, min, max, step))}
      >
        +
      </Button>
    </div>
  );
}

const PATH = 'block max-w-[280px] truncate rounded-lg border border-default bg-surface px-2.5 py-1.5 text-xs text-secondary';

/**
 * One row of the settings page: a label with its help text on the left, and
 * the control for `field.kind` on the right.
 *
 * Owns per-instance, ephemeral UI state: `dragging` for sliders, and for a
 * `hotkey` field its own `capture`/`listening`/`heard` triple, so several
 * hotkey rows stay independent.
 */
export function Field({
  field,
  config,
  monitors,
  problem = null,
  inactive = null,
  onset,
  onpickfolder,
  onpicksound,
  ontestsound,
}: {
  field: FieldDef;
  config: Record<string, unknown>;
  monitors: Monitor[];
  /** Why this setting is not currently working, or null. Shown in the danger colour. */
  problem?: string | null;
  /** Why this row has no effect right now, or null. The row stays usable, only dimmed. */
  inactive?: string | null;
  onset: (key: string, value: unknown) => void;
  onpickfolder: () => void;
  onpicksound: () => void;
  ontestsound: () => void;
}) {
  const value = config[field.key];

  // Hotkey live test. Only the row whose press reaches the daemon as
  // `hotkey_pressed` (descriptor's `liveTest`) subscribes.
  const [listening, setListening] = useState(false);
  const [heard, setHeard] = useState(false);
  const [capture, setCapture] = useState<string | null>(null);
  const testable = field.kind === 'hotkey' && field.liveTest === true;
  const listeningRef = useRef(listening);
  listeningRef.current = listening;

  useEffect(() => {
    if (!testable) return;
    const unlisten = onDaemonEvent((event) => {
      if (event.event === 'hotkey_pressed' && listeningRef.current) setHeard(true);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [testable]);

  function captureHotkey(e: React.KeyboardEvent) {
    // Unconditional, before the combination is worked out: a bare Alt press
    // still reaches this handler, and letting that one through would move
    // focus to the window menu mid-recording.
    e.preventDefault();
    const combo = comboFromEvent(e.nativeEvent);
    if (combo !== null) setCapture(combo);
  }

  function saveHotkey() {
    if (capture === null) return;
    const target = hotkeySaveTarget(field, capture);
    onset(target.key, target.value);
    setCapture(null);
  }

  // Slider: the value under the thumb is display-only while dragging; the
  // daemon is told once on release (pointer up, key up or blur), not on every
  // movement. Cleared whenever the authoritative value lands -- accepted or
  // refused-and-reloaded -- so a refused change never leaves a stale thumb.
  const [dragging, setDragging] = useState<number | null>(null);
  const draggingRef = useRef<number | null>(null);
  useEffect(() => {
    draggingRef.current = null;
    setDragging(null);
  }, [value]);
  const shown = dragging ?? Number(value ?? 0);

  function commitDrag() {
    const v = draggingRef.current;
    if (v === null) return;
    if (v === Number(value ?? 0)) {
      draggingRef.current = null;
      setDragging(null);
      return;
    }
    // Released: the shown value stays until the daemon's answer lands, but a
    // following key-up or blur must not send it a second time.
    draggingRef.current = null;
    onset(field.key, v);
  }

  // A `tier` row showing its Custom box. Sticky once set, so typing 8000 into
  // the box does not snap the dropdown back onto the Medium preset mid-edit.
  const [custom, setCustom] = useState(false);
  const tier = custom ? CUSTOM_TIER : tierFor(field, value);

  function pickTier(next: string) {
    if (next === CUSTOM_TIER) {
      setCustom(true);
      return;
    }
    setCustom(false);
    onset(field.key, Number(next));
  }

  const monitorOptions = monitors.map((m) => ({
    value: String(m.index),
    label: `${m.name} – ${m.width}×${m.height} (${m.adapter})`,
  }));

  const left = (
    <>
      <span>{field.help}</span>
      {field.more && (
        <details className="group mt-1">
          <summary className={`inline-block cursor-pointer text-xs text-tertiary underline underline-offset-2 transition-colors duration-200 ${EASE} hover:text-default`}>
            Détails
          </summary>
          <span className="mt-1 block">{field.more}</span>
        </details>
      )}
      {inactive && <span className="mt-1 block italic">{inactive}</span>}
      {problem && <span className="mt-1.5 block text-[var(--color-text-danger)]">{problem}</span>}
    </>
  );

  let control: ReactNode = null;
  switch (field.kind) {
    case 'bool':
      control = (
        <Switch
          checked={Boolean(value)}
          label={<span className="sr-only">{field.label}</span>}
          onCheckedChange={(next) => onset(field.key, next)}
        />
      );
      break;

    case 'select':
      control = (
        <div role="group" aria-label={field.label}>
          {field.dynamic === 'monitors' ? (
            <Select
              block={false}
              size="sm"
              value={String(value ?? 0)}
              options={monitorOptions}
              onChange={(o) => onset(field.key, Number(o.value))}
            />
          ) : (
            <Select
              block={false}
              size="sm"
              value={String(value ?? '')}
              options={field.options ?? []}
              onChange={(o) => onset(field.key, field.numeric ? Number(o.value) : o.value)}
            />
          )}
        </div>
      );
      break;

    case 'number':
      control = (
        <Stepper
          value={Number(value ?? 0)}
          min={field.min ?? 0}
          max={field.max ?? 0}
          unit={UNITS[field.key]}
          label={field.label}
          onchange={(v) => onset(field.key, v)}
        />
      );
      break;

    case 'tier':
      control = (
        <>
          <div role="group" aria-label={field.label}>
            <Select block={false} size="sm" value={tier} options={tierOptions(field)} onChange={(o) => pickTier(o.value)} />
          </div>
          {tier === CUSTOM_TIER && (
            <Stepper
              value={Number(value ?? 0)}
              min={field.min ?? 0}
              max={field.max ?? 0}
              unit={UNITS[field.key]}
              label={`${field.label}, valeur personnalisée`}
              onchange={(v) => onset(field.key, v)}
            />
          )}
        </>
      );
      break;

    case 'slider':
      control = (
        <div
          role="group"
          aria-label={field.label}
          className="w-[240px]"
          onPointerUp={commitDrag}
          onKeyUp={commitDrag}
          onBlur={commitDrag}
        >
          <Slider
            value={shown}
            min={field.min ?? 0}
            max={field.max ?? 100}
            step={1}
            unit="%"
            onChange={(v) => {
              draggingRef.current = v;
              setDragging(v);
            }}
          />
        </div>
      );
      break;

    case 'folder':
      // `clip_dir_resolved`, not `clip_dir`: the config key is empty by default
      // and an empty box tells the user nothing about where their clips are.
      // Read-only: the daemon owns the picker. Reset writes `clip_dir` itself
      // (the empty string), because "back to the default" is a value the
      // picker cannot express.
      control = (
        <>
          <span className={PATH} title={String(config['clip_dir_resolved'] ?? '')}>{String(config['clip_dir_resolved'] ?? '')}</span>
          <Button color="secondary" variant="outline" size="sm" onClick={onpickfolder}>Choisir…</Button>
          <Button color="secondary" variant="ghost" size="sm" disabled={!value} onClick={() => onset(field.key, '')}>Rétablir</Button>
        </>
      );
      break;

    case 'sound':
      control = (
        <>
          <span className={`${PATH} ${config['clip_sound'] ? '' : 'opacity-45'}`}>
            {String(value ?? '') || 'Son intégré de Clippy'}
          </span>
          <Button color="secondary" variant="outline" size="sm" disabled={!config['clip_sound']} onClick={onpicksound}>Choisir…</Button>
          <Button color="secondary" variant="ghost" size="sm" disabled={!config['clip_sound']} onClick={ontestsound}>Tester</Button>
          <Button color="secondary" variant="ghost" size="sm" disabled={!config['clip_sound'] || !value} onClick={() => onset(field.key, '')}>Rétablir</Button>
        </>
      );
      break;

    case 'apps':
      control = (
        <AppPicker
          selected={Array.isArray(value) ? (value as string[]) : []}
          emptyText={field.picker?.empty}
          addText={field.picker?.add}
          onchange={(next) => onset(field.key, next)}
        />
      );
      break;

    case 'hotkey':
      control = (
        <>
          <KeycapInput
            combo={capture ?? hotkeySeed(field, config)}
            capturing={capture !== null}
            label={field.label}
            oncapture={captureHotkey}
            oncancel={() => setCapture(null)}
            onstart={() => setCapture(hotkeySeed(field, config))}
          />
          {capture && capture !== value && (
            <Button color="primary" size="sm" onClick={saveHotkey}>Enregistrer</Button>
          )}
          {testable && (
            <>
              <Button
                color="secondary"
                variant="ghost"
                size="sm"
                onClick={() => {
                  setListening(!listening);
                  setHeard(false);
                }}
              >
                {listening ? 'Arrêter le test' : 'Tester'}
              </Button>
              {listening && (
                <span className={`text-xs transition-colors duration-200 ${heard ? 'text-[var(--color-text-success)]' : 'text-secondary'}`}>
                  {heard ? 'Reçu par Clippy.' : 'Appuyer sur le raccourci…'}
                </span>
              )}
            </>
          )}
        </>
      );
      break;
  }

  return (
    <Row title={field.label} dimmed={inactive !== null} left={left}>
      {control}
    </Row>
  );
}
