import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState } from 'react';
import { Maximize, Minus, Restore, X } from './icons';

/** The window's caption buttons, in OpenAI's icon shapes like the rest of the app. */
const SHAPES = { minimize: Minus, maximize: Maximize, restore: Restore, close: X } as const;

function Caption({ shape, label, onClick, danger }: { shape: keyof typeof SHAPES; label: string; onClick: () => void; danger?: boolean }) {
  const Icon = SHAPES[shape];
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`grid h-full w-11 place-items-center text-secondary transition-colors duration-150 hover:text-default ${
        danger ? 'hover:bg-[#e81123] hover:text-white' : 'hover:bg-[var(--color-background-primary-soft-alpha)]'
      }`}>
      <Icon className="size-[18px]" aria-hidden="true" />
    </button>
  );
}

export function WindowControls() {
  const [win] = useState(getCurrentWindow);
  const [maximized, setMaximized] = useState(false);

  // The window can be maximized without these buttons (double-click on the drag
  // region, Win+Up, dragging to the top edge), so the state is read from the
  // window and never inferred from our own clicks.
  useEffect(() => {
    let alive = true;
    const sync = () => void win.isMaximized().then((v) => alive && setMaximized(v));
    sync();
    const un = win.onResized(sync);
    return () => {
      alive = false;
      void un.then((fn) => fn());
    };
  }, [win]);

  return (
    <div className="flex self-stretch">
      <Caption shape="minimize" label="Réduire" onClick={() => void win.minimize()} />
      <Caption
        shape={maximized ? 'restore' : 'maximize'}
        label={maximized ? 'Restaurer' : 'Agrandir'}
        onClick={() => void win.toggleMaximize()}
      />
      <Caption shape="close" label="Fermer" danger onClick={() => void win.close()} />
    </div>
  );
}
