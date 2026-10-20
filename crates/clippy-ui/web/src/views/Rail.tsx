import { Camera, Clips, Gamepad, Settings } from '../components/icons';
import { app, useApp } from '../lib/app';
import type { View } from '../lib/state';

const ITEMS: { view: View; label: string; Icon: typeof Clips }[] = [
  { view: 'grid', label: 'Clips', Icon: Clips },
  { view: 'shots', label: 'Captures d’écran', Icon: Camera },
  { view: 'games', label: 'Jeux', Icon: Gamepad },
  { view: 'settings', label: 'Paramètres', Icon: Settings },
];

export function Rail() {
  const a = useApp();
  return (
    <nav aria-label="Navigation" className="flex w-52 shrink-0 flex-col gap-1 border-r border-subtle bg-surface-secondary p-3">
      {ITEMS.map(({ view, label, Icon }) => {
        const current = a.view === view || (view === 'grid' && a.view === 'clip');
        return (
          <button
            key={view}
            type="button"
            aria-current={current ? 'page' : undefined}
            onClick={() => (app.view = view)}
            className={`flex items-center gap-2.5 whitespace-nowrap rounded-lg px-3 py-2 text-left text-sm transition-colors duration-150 ${
              current
                ? 'bg-[var(--color-background-primary-soft-alpha)] font-semibold text-default'
                : 'text-secondary hover:bg-[var(--color-background-primary-soft-alpha)] hover:text-default'
            }`}>
            <Icon className="size-[18px]" />
            {label}
          </button>
        );
      })}
      <div className="mt-auto px-3 text-xs text-tertiary tabular-nums">{a.status?.version ?? ''}</div>
    </nav>
  );
}
