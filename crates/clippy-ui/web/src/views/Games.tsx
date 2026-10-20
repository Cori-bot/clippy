import { useEffect, useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { GameRow } from '../components/GameRow';
import { WindowPicker } from '../components/WindowPicker';
import { app, useApp } from '../lib/app';
import { gameLabel } from '../lib/games';

export function Games() {
  const a = useApp();
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    void app.loadGames();
    void app.refreshStatus();
  }, []);

  const current = a.status?.game ?? null;
  const currentName = current ? a.status?.game_name || gameLabel({ exe: current, name: '' }) : null;

  return (
    <div className="mx-auto flex max-w-[720px] flex-col gap-6">
      <header>
        <h1 className="heading-lg mb-1">Jeux</h1>
        <p className="text-sm text-secondary">
          Clippy reconnaît tout seul les jeux que tu lances. Ajoute ici ceux qu’il ignore et règle les pistes audio de
          chacun. Les clips sont classés dans un dossier par jeu.
        </p>
        <p className="mt-2 text-sm text-secondary">
          Le mode « Jeux uniquement » se règle dans{' '}
          <button type="button" className="text-info underline" onClick={() => (app.view = 'settings')}>
            Paramètres › Enregistrement
          </button>
          .
        </p>
      </header>

      <section aria-labelledby="current-game" className="rounded-xl border border-subtle bg-surface p-4">
        <h2 id="current-game" className="mb-2 text-xs font-semibold tracking-wider text-secondary uppercase">
          Jeu actuel
        </h2>
        {currentName ? (
          <div className="flex items-baseline gap-2">
            <span className="size-2 shrink-0 rounded-full bg-success" aria-hidden />
            <span className="text-base font-semibold text-default">{currentName}</span>
            <span className="text-xs text-tertiary">{current}</span>
          </div>
        ) : (
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm">
            <span className="text-default">Aucun jeu détecté</span>
            <span className="text-secondary">
              Tu ne trouves pas ton jeu ?{' '}
              <button type="button" className="text-info underline" onClick={() => setAdding(true)}>
                Ajoute-le !
              </button>
            </span>
          </div>
        )}
      </section>

      <section aria-labelledby="added-games">
        <div className="mb-2 flex items-center">
          <h2 id="added-games" className="text-xs font-semibold tracking-wider text-secondary uppercase">
            Jeux ajoutés
          </h2>
          <Button className="ml-auto" color="primary" size="sm" onClick={() => setAdding(true)}>
            Ajouter un jeu
          </Button>
        </div>
        {a.games.length === 0 ? (
          <p className="rounded-xl border border-dashed border-default px-4 py-8 text-center text-sm text-secondary">
            Aucun jeu pour l’instant. Lance un jeu : il apparaîtra ici.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {a.games.map((g) => (
              <GameRow key={g.exe} game={g} />
            ))}
          </ul>
        )}
      </section>

      <WindowPicker
        open={adding}
        onOpenChange={setAdding}
        title="Ajouter un jeu"
        load={() => app.listWindows()}
        taken={a.games.map((g) => g.exe)}
        onPick={(exe, name) => void app.setGame(exe, { name })}
      />
    </div>
  );
}
