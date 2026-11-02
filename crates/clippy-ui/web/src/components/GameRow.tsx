import { useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Switch } from '@openai/apps-sdk-ui/components/Switch';
import { Dialog } from './ui/Dialog';
import { GameProfile } from './GameProfile';
import { ChevronDown, Trash } from './icons';
import { app } from '../lib/app';
import { gameLabel, lastPlayedText } from '../lib/games';
import type { GameEntry } from '../lib/types';

/** One added game: name, last session, an on/off switch and an expandable profile. */
export function GameRow({ game }: { game: GameEntry }) {
  const [open, setOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const name = gameLabel(game);
  const panelId = `profile-${game.exe}`;

  return (
    <li className="rounded-xl border border-subtle bg-surface">
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="truncate text-sm font-semibold text-default">{name}</span>
            <span className="shrink-0 text-xs text-tertiary">{game.exe}</span>
          </div>
          <div className="text-xs text-secondary">Dernière session de jeu : {lastPlayedText(game.last_played)}</div>
        </div>
        <Switch
          checked={game.enabled}
          aria-label={`Enregistrer ${name}`}
          onCheckedChange={(enabled) => void app.setGame(game.exe, { enabled })}
        />
        <Button
          color="secondary"
          variant="outline"
          size="sm"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => setOpen(!open)}>
          Profil
          <ChevronDown className={`transition-transform duration-150 ${open ? 'rotate-180' : ''}`} />
        </Button>
        <Button
          color="secondary"
          variant="ghost"
          size="sm"
          uniform
          aria-label={`Retirer ${name}`}
          onClick={() => setRemoving(true)}>
          <Trash />
        </Button>
      </div>
      {open && (
        <div id={panelId} className="px-4 pb-4">
          <GameProfile game={game} />
        </div>
      )}
      <Dialog
        open={removing}
        onOpenChange={setRemoving}
        title={`Retirer ${name} ?`}
        actions={
          <>
            <Button color="secondary" variant="outline" onClick={() => setRemoving(false)}>
              Annuler
            </Button>
            <Button
              color="danger"
              onClick={() => {
                setRemoving(false);
                void app.removeGame(game.exe);
              }}>
              Retirer
            </Button>
          </>
        }>
        {game.manual
          ? 'Ce jeu ne sera plus reconnu. Les clips déjà enregistrés sont conservés.'
          : 'Ce jeu disparaît de la liste et ne sera plus détecté, même en plein écran, tant qu’il n’est pas ajouté à nouveau. Les clips déjà enregistrés sont conservés.'}
      </Dialog>
    </li>
  );
}
