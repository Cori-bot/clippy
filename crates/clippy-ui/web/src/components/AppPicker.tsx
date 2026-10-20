import { useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { Menu } from '@openai/apps-sdk-ui/components/Menu';
import { call } from '../lib/ipc';
import { app } from '../lib/app';
import { MAX_AUDIO_APPS, appLabel, withApp, withoutApp } from '../lib/settings';
import type { AudioApp } from '../lib/types';
import { Close } from './icons';

/**
 * The applications whose sound is recorded, as the executable names the
 * daemon matches against, with a way to add one from what is making sound
 * right now or by typing its name.
 *
 * Empty means "everything the PC plays", and is said in words, because an
 * empty list of chips would otherwise read as "nothing is recorded".
 */
export function AppPicker({
  selected,
  onchange,
  emptyText = 'Tout le son du PC',
  addText = 'Ajouter une application',
}: {
  selected: string[];
  onchange: (next: string[]) => void;
  /** What an empty list means, in words. */
  emptyText?: string;
  addText?: string;
}) {
  const [apps, setApps] = useState<AudioApp[]>([]);
  const [typing, setTyping] = useState(false);
  const [typed, setTyped] = useState('');

  const available = apps.filter((a) => !selected.includes(a.exe));

  // Fetched on every opening: what is playing is the whole point of the list.
  async function fetchApps() {
    try {
      setApps((await call<{ apps: AudioApp[] }>('audio.apps')).apps);
    } catch (e) {
      app.toast('error', String(e));
      setApps([]);
    }
  }

  function commitTyped() {
    const next = withApp(selected, typed);
    setTyping(false);
    setTyped('');
    if (next !== selected) onchange(next);
  }

  function label(exe: string): string {
    return apps.find((a) => a.exe === exe)?.name ?? appLabel(exe);
  }

  const full = selected.length >= MAX_AUDIO_APPS;

  return (
    <div className="flex flex-wrap items-center justify-end gap-1.5">
      {selected.length === 0 ? (
        <span className="mr-1 text-sm text-secondary">{emptyText}</span>
      ) : (
        selected.map((exe) => (
          <span key={exe} className="inline-flex items-center gap-1 rounded-lg border border-default bg-surface py-0.5 pr-0.5 pl-2.5 text-sm text-default">
            {label(exe)}
            <Button
              color="secondary"
              variant="ghost"
              size="3xs"
              uniform
              pill={false}
              aria-label={`Retirer ${label(exe)}`}
              onClick={() => onchange(withoutApp(selected, exe))}
            >
              <Close />
            </Button>
          </span>
        ))
      )}

      {typing ? (
        <Input
          size="sm"
          className="w-[150px]"
          autoFocus
          placeholder="spotify.exe"
          aria-label="Nom de l’exécutable de l’application"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commitTyped();
            else if (e.key === 'Escape') {
              setTyping(false);
              setTyped('');
            }
          }}
          onBlur={commitTyped}
        />
      ) : (
        <Menu onOpen={() => void fetchApps()}>
          <Menu.Trigger disabled={full}>
            <Button color="secondary" variant="outline" size="sm" disabled={full}>
              {full ? `${MAX_AUDIO_APPS} maximum` : addText}
            </Button>
          </Menu.Trigger>
          <Menu.Content align="end" minWidth={220}>
            {available.map((a) => (
              <Menu.Item key={a.exe} onSelect={() => onchange(withApp(selected, a.exe))}>
                {a.playing ? `${a.name} · joue du son` : a.name}
              </Menu.Item>
            ))}
            {available.length > 0 && <Menu.Separator />}
            <Menu.Item onSelect={() => setTyping(true)}>Autre application…</Menu.Item>
          </Menu.Content>
        </Menu>
      )}
    </div>
  );
}
