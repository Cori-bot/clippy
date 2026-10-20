import { useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Switch } from '@openai/apps-sdk-ui/components/Switch';
import { WindowPicker } from './WindowPicker';
import { Close } from './icons';
import { app } from '../lib/app';
import { call } from '../lib/ipc';
import { gameLabel, withMic, withProfileApp, withoutProfileApp } from '../lib/games';
import { MAX_AUDIO_APPS } from '../lib/settings';
import type { AudioApp, GameEntry } from '../lib/types';

/**
 * The tracks of one game's clips: the game's own sound (always recorded,
 * fixed), then the extra sources the user adds, each its own track. Every
 * change is saved at once with the whole `profile`.
 */
export function GameProfile({ game }: { game: GameEntry }) {
  const [adding, setAdding] = useState(false);
  const { profile } = game;
  const full = profile.apps.length >= MAX_AUDIO_APPS;

  const save = (next: GameEntry['profile']) => void app.setGame(game.exe, { profile: next });

  async function loadAudioApps() {
    try {
      const { apps } = await call<{ apps: AudioApp[] }>('audio.apps');
      return apps.map((a) => ({ exe: a.exe, name: a.name, title: a.playing ? 'joue du son' : '' }));
    } catch (e) {
      app.toast('error', String(e));
      return [];
    }
  }

  return (
    <div className="flex flex-col gap-3 border-t border-subtle pt-4">
      <div>
        <h3 className="text-sm font-semibold text-default">Pistes audio de ce jeu</h3>
        <p className="mt-0.5 text-sm text-secondary">
          Le son du jeu est toujours enregistré. Ajoute d’autres sources : chacune devient une piste séparée dans le clip.
        </p>
      </div>

      <ul className="flex flex-col divide-y divide-[var(--color-border-subtle)] rounded-xl border border-subtle">
        <li className="flex items-center gap-3 px-3 py-2 text-sm">
          <span className="font-medium text-default">{gameLabel(game)}</span>
          <span className="ml-auto text-xs text-tertiary">Toujours enregistré</span>
        </li>
        {profile.apps.map((a) => (
          <li key={a.exe} className="flex items-center gap-3 px-3 py-2 text-sm">
            <span className="text-default">{a.name}</span>
            <span className="text-xs text-tertiary">{a.exe}</span>
            <Button
              className="ml-auto"
              color="secondary"
              variant="ghost"
              size="3xs"
              uniform
              pill={false}
              aria-label={`Retirer ${a.name}`}
              onClick={() => save(withoutProfileApp(profile, a.exe))}>
              <Close />
            </Button>
          </li>
        ))}
      </ul>

      <div className="flex flex-wrap items-center gap-3">
        <Button color="secondary" variant="outline" size="sm" disabled={full} onClick={() => setAdding(true)}>
          {full ? `${MAX_AUDIO_APPS} maximum` : 'Ajouter une application'}
        </Button>
        <Switch
          className="ml-auto"
          checked={profile.mic}
          label="Microphone"
          onCheckedChange={(on) => save(withMic(profile, on))}
        />
      </div>

      <WindowPicker
        open={adding}
        onOpenChange={setAdding}
        title="Ajouter une source audio"
        load={loadAudioApps}
        taken={profile.apps.map((a) => a.exe)}
        onPick={(exe, name) => save(withProfileApp(profile, { exe, name }))}
      />
    </div>
  );
}
