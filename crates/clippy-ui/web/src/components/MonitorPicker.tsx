import type { Monitor } from '../lib/types';
import { isPortrait, isPrimary, monitorLayout, monitorLabel } from '../lib/monitors';

const EASE = 'ease-[cubic-bezier(0.22,1,0.36,1)]';

/**
 * The screens as Windows arranges them, to scale: each one is a button showing
 * its number, its size and whether it stands upright, so the choice is made by
 * looking rather than by reading a device path like `\\.\DISPLAY1`.
 */
export function MonitorPicker({ monitors, value, label, onchange }: {
  monitors: Monitor[];
  value: number;
  label: string;
  onchange: (index: number) => void;
}) {
  if (monitors.length === 0) {
    return <span className="justify-self-end text-xs text-secondary">Aucun écran détecté</span>;
  }
  const layout = monitorLayout(monitors, 260, 110);
  const current = monitors.find((m) => m.index === value);
  return (
    // `contents`: the screens and the detail are the middle and right columns
    // of the settings row, so the screens sit in the middle of it.
    <div className="contents">
      <div role="radiogroup" aria-label={label} className="relative" style={{ width: layout.width, height: layout.height }}>
        {layout.boxes.map(({ monitor, x, y, w, h }) => {
          const selected = monitor.index === value;
          return (
            <button
              key={monitor.index}
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={monitorLabel(monitor)}
              title={monitorLabel(monitor)}
              onClick={() => onchange(monitor.index)}
              className={`absolute flex cursor-pointer flex-col items-center justify-center rounded-md border p-0 text-default transition-[background-color,border-color,box-shadow] duration-200 ${EASE} ${
                selected
                  ? 'border-transparent bg-surface-elevated shadow-[inset_0_0_0_2px_var(--color-ring)]'
                  : 'border-default bg-surface-secondary hover:border-strong'
              }`}
              style={{ left: x, top: y, width: w, height: h }}>
              <span className="text-sm font-semibold leading-none">{monitor.index + 1}</span>
              {h >= 34 && w >= 44 && (
                <span className="mt-1 text-[10px] leading-none text-secondary tabular-nums">
                  {monitor.width}×{monitor.height}
                </span>
              )}
            </button>
          );
        })}
      </div>
      {current && (
        <div className="flex flex-col items-end gap-0.5 justify-self-end whitespace-nowrap text-right text-xs text-secondary" aria-live="polite">
          <b className="text-sm font-medium text-default">Écran {current.index + 1}</b>
          <span className="tabular-nums">{current.width}×{current.height}</span>
          <span>
            {isPortrait(current) ? 'Vertical' : 'Horizontal'}
            {isPrimary(current) ? ' · principal' : ''}
          </span>
        </div>
      )}
    </div>
  );
}
