import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Slider } from '@openai/apps-sdk-ui/components/Slider';
import type { KeyboardEvent } from 'react';
import { MAX_GAIN, type EditableTrack, type TrackState } from '../lib/tracks';
import { Volume, VolumeMute } from './icons';

/**
 * One row per editable audio track, directly under the timeline: name, mute
 * toggle, level (0-200 %). Presentational; `ClipPage` owns the state because
 * the export needs it.
 */
export function TrackRows({
  tracks,
  states,
  loading,
  error,
  onChange,
}: {
  tracks: EditableTrack[];
  states: TrackState[];
  /** The daemon is still preparing the per-track files. */
  loading: boolean;
  /** The files could not be prepared: levels still apply to the export only. */
  error: boolean;
  onChange: (index: number, next: TrackState) => void;
}) {
  if (tracks.length === 0) return null;

  // The sliders handle the arrows themselves; keep them from also stepping
  // clips or the master volume through the page's window-level shortcuts.
  const stopArrows = (e: KeyboardEvent) => {
    if (e.key.startsWith('Arrow')) e.stopPropagation();
  };

  return (
    <section aria-label="Pistes audio" className="mt-2 grid gap-1" onKeyDown={stopArrows}>
      {(loading || error) && (
        <p className="text-xs text-tertiary">
          {loading
            ? 'Préparation des pistes audio…'
            : 'Aperçu séparé indisponible : le son mixé est lu. Les réglages s’appliquent à l’export.'}
        </p>
      )}
      {tracks.map((t, i) => {
        const s = states[i] ?? { muted: false, gain: 100 };
        return (
          <div key={t.index} className="flex items-center gap-3">
            <span className="w-8 shrink-0" aria-hidden="true" />
            <span className="w-40 shrink-0 truncate text-sm text-secondary" title={t.name}>
              {t.name}
            </span>
            <Button
              color="secondary"
              variant="ghost"
              size="sm"
              uniform
              disabled={loading}
              aria-pressed={s.muted}
              aria-label={`${s.muted ? 'Réactiver' : 'Couper'} la piste ${t.name}`}
              className="shrink-0 transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]"
              onClick={() => onChange(i, { ...s, muted: !s.muted })}>
              {s.muted ? <VolumeMute className="size-4" /> : <Volume className="size-4" />}
            </Button>
            <div
              role="group"
              aria-label={`Volume de la piste ${t.name}`}
              title="Au-delà de 100 %, le gain n’est appliqué qu’à l’export (l’aperçu est limité à 100 %)."
              className={`w-64 max-w-full ${s.muted ? 'opacity-50' : ''}`}>
              <Slider
                value={s.gain}
                min={0}
                max={MAX_GAIN}
                step={5}
                unit="%"
                resetValue={100}
                disabled={loading}
                onChange={(v) => onChange(i, { ...s, gain: v })}
              />
            </div>
          </div>
        );
      })}
    </section>
  );
}
