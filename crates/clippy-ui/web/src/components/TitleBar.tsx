import { Button } from '@openai/apps-sdk-ui/components/Button';
import { app, useApp } from '../lib/app';
import { Logo } from './Logo';
import { WindowControls } from './WindowControls';

export function TitleBar() {
  const a = useApp();
  const pct =
    a.status && a.status.ring_seconds_total > 0
      ? Math.min(100, (a.ringUsed / a.status.ring_seconds_total) * 100)
      : 0;
  const on = a.armed;
  const waiting = a.waiting && !a.armed;
  const title = waiting
    ? 'Le replay démarre dès qu’un jeu est au premier plan. Cliquer pour le désactiver.'
    : on
      ? `${a.status?.game ? `Jeu détecté : ${a.status.game}. ` : ''}Cliquer pour désactiver le replay.`
      : undefined;

  return (
    <header className="flex h-10 shrink-0 items-center gap-3 border-b border-subtle bg-surface-secondary pl-3">
      <Logo />
      <span className="text-sm font-semibold tracking-wide">CLIPPY</span>

      <Button
        color={on ? 'success' : 'secondary'}
        variant={on ? 'soft' : 'outline'}
        size="xs"
        aria-label="Replay"
        aria-pressed={on || a.waiting}
        title={title}
        disabled={!a.connected}
        onClick={() => void app.toggleArm()}>
        <span
          className={`size-1.5 rounded-full ${on ? 'bg-[var(--color-text-success)]' : waiting ? 'ring-[1.5px] ring-inset ring-[var(--color-text-success)]' : 'bg-[var(--color-text-tertiary)]'}`}
        />
        {on ? 'Replay actif' : waiting ? 'Replay prêt · attend un jeu' : 'Activer le replay'}
        {on && a.status?.game && <span className="max-w-[16ch] truncate font-normal opacity-80">{a.status.game}</span>}
        {on && (
          <>
            <span className="h-[3px] w-11 overflow-hidden rounded-sm bg-[var(--color-border-strong)]" aria-hidden="true">
              <i
                className="block h-full w-full origin-left bg-[var(--color-text-success)] transition-transform duration-200 ease-linear"
                style={{ transform: `scaleX(${pct / 100})` }}
              />
            </span>
            <span className="tabular-nums opacity-85">
              {a.ringUsed.toFixed(0)}/{a.status?.ring_seconds_total.toFixed(0) ?? '0'}&nbsp;s
            </span>
          </>
        )}
      </Button>

      {/* The whole remaining width drags the window; Tauri handles double-click-to-maximize itself. */}
      <div className="flex-1 self-stretch" data-tauri-drag-region />
      <WindowControls />
    </header>
  );
}
