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
    <section aria-label="Pistes audio" className="editor-audio-tracks" onKeyDown={stopArrows}>
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
          <div key={t.index} className="editor-audio-track">
            <span className="min-w-0 truncate text-xs text-secondary" title={t.name}>
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
              title="Gain de la piste, appliqué à l’aperçu et à l’export."
              className={`col-span-2 min-w-0 ${s.muted ? 'opacity-50' : ''}`}>
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
