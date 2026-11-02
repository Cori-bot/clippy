import { Camera, Clips, Gamepad, Settings } from '../components/icons';
import { app, useApp } from '../lib/app';
import type { View } from '../lib/state';
import { ReplayControl } from '../components/ReplayControl';
import { Logo } from '../components/Logo';
import { ExportJobPanel } from '../components/ExportJobPanel';

/** `motion` is the hover animation (index.css): each icon moves like its object. */
const ITEMS: { view: View; label: string; Icon: typeof Clips; motion: string }[] = [
  { view: 'grid', label: 'Clips', Icon: Clips, motion: 'motion-roll' },
  { view: 'shots', label: 'Captures d’écran', Icon: Camera, motion: 'motion-flash' },
  { view: 'games', label: 'Jeux', Icon: Gamepad, motion: 'motion-wiggle' },
  { view: 'settings', label: 'Paramètres', Icon: Settings, motion: 'motion-spin' },
];

export function Rail() {
  const a = useApp();
  return (
    <nav aria-label="Navigation" className="flex w-48 shrink-0 flex-col gap-1 overflow-y-auto border-r border-subtle bg-surface-secondary p-3">
      <div className="mb-5 flex items-center gap-2.5 px-2 py-2">
        <Logo height={25} /><span className="text-lg font-semibold">Clippy</span>
      </div>
      {ITEMS.map(({ view, label, Icon, motion }) => {
        const current = a.view === view || (view === 'grid' && a.view === 'clip');
        return (
          <button
            key={view}
            type="button"
            aria-current={current ? 'page' : undefined}
            onClick={() => (app.view = view)}
            className={`motion-control flex items-center gap-2.5 whitespace-nowrap rounded-lg px-3 py-2 text-left text-sm transition-colors duration-150 ${
              current
                ? 'bg-[var(--color-background-primary-soft-alpha)] font-semibold text-default'
                : 'text-secondary hover:bg-[var(--color-background-primary-soft-alpha)] hover:text-default'
            }`}>
            <span className={`motion-icon ${motion}`}><Icon className="size-[18px]" /></span>
            {label}
          </button>
        );
      })}
      <ExportJobPanel />
      <ReplayControl />
      <div className="mt-3 px-2 text-xs text-tertiary tabular-nums">{a.status?.version ?? ''}</div>
    </nav>
  );
}
