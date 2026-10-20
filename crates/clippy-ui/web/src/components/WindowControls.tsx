import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState } from 'react';

/** The window's caption buttons: Windows' own shapes, because that is where people expect exactly those. */
const SHAPES = {
  minimize: 'M4.5 12h15',
  maximize: 'M5.25 5.25h13.5v13.5H5.25z',
  restore: 'M7.5 7.5V5.25h11.25V16.5H16.5M5.25 7.5H16.5v11.25H5.25z',
  close: 'M5.25 5.25l13.5 13.5M18.75 5.25l-13.5 13.5',
} as const;

function Caption({ shape, label, onClick, danger }: { shape: keyof typeof SHAPES; label: string; onClick: () => void; danger?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`grid h-full w-11 place-items-center text-secondary transition-colors duration-150 hover:text-default ${
        danger ? 'hover:bg-[#e81123]' : 'hover:bg-[var(--color-background-primary-soft-alpha)]'
      }`}>
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d={SHAPES[shape]} />
      </svg>
    </button>
  );
}

export function WindowControls() {
  const win = getCurrentWindow();
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
