import { useEffect, useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { Dialog } from './ui/Dialog';
import { exeFromInput, filterWindows } from '../lib/games';
import { appLabel } from '../lib/settings';

export type PickItem = { exe: string; name: string; title: string };

/**
 * A searchable list of running applications, plus a free entry for anything
 * not listed. Used to add a game (from `windows.list`) and to add an extra
 * audio source to a game profile (from `audio.apps`): the caller supplies the
 * loader, so the dialog does not care which one it is.
 *
 * Fetched on every opening, since what is running is the whole point.
 */
export function WindowPicker({
  open,
  onOpenChange,
  title,
  load,
  taken,
  onPick,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  load: () => Promise<PickItem[]>;
  /** Executables already chosen; hidden from the list. */
  taken: string[];
  onPick: (exe: string, name: string) => void;
}) {
  const [items, setItems] = useState<PickItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState('');
  const [typed, setTyped] = useState('');

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setTyped('');
    setLoading(true);
    let live = true;
    load()
      .then((list) => live && setItems(list))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // `load` is a fresh closure each render; opening is the only trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const shown = filterWindows(items, taken, query);
  const manual = exeFromInput(typed);

  function pick(exe: string, name: string) {
    onPick(exe, name);
    onOpenChange(false);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      actions={
        <Button color="secondary" variant="outline" onClick={() => onOpenChange(false)}>
          Annuler
        </Button>
      }>
      <div className="flex flex-col gap-3">
        <Input
          size="sm"
          type="search"
          autoFocus
          placeholder="Rechercher une application ouverte"
          aria-label="Rechercher une application ouverte"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <ul className="-mx-1 flex max-h-64 flex-col overflow-auto" aria-label="Applications ouvertes">
          {shown.map((w) => (
            <li key={w.exe}>
              <button
                type="button"
                onClick={() => pick(w.exe, w.name || appLabel(w.exe))}
                className="flex w-full items-baseline gap-2 rounded-lg px-2 py-1.5 text-left text-sm text-default transition-colors duration-150 hover:bg-[var(--color-background-primary-soft-alpha)] focus-visible:bg-[var(--color-background-primary-soft-alpha)] focus-visible:outline-none">
                <span className="truncate">{w.name || appLabel(w.exe)}</span>
                {w.title && w.title !== w.name && <span className="truncate text-xs text-tertiary">{w.title}</span>}
                <span className="ml-auto shrink-0 text-xs text-tertiary">{w.exe}</span>
              </button>
            </li>
          ))}
          {shown.length === 0 && (
            <li className="px-2 py-3 text-sm text-tertiary">
              {loading ? 'Recherche des applications…' : 'Aucune application ouverte ne correspond.'}
            </li>
          )}
        </ul>
        <div className="border-t border-subtle pt-3">
          <label className="mb-1.5 block text-xs text-secondary" htmlFor="picker-other">
            Autre application…
          </label>
          <div className="flex gap-2">
            <Input
              id="picker-other"
              size="sm"
              className="flex-1"
              placeholder="MonJeu.exe"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && manual) pick(manual, appLabel(manual));
              }}
            />
            <Button color="primary" size="sm" disabled={!manual} onClick={() => pick(manual, appLabel(manual))}>
              Ajouter
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
