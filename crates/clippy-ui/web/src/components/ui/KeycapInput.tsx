import type { KeyboardEvent } from 'react';
import { formatCombo } from '../../lib/ui';

/**
 * Shortcut recorder. Always a button: click it, then press the combination.
 *
 * Capturing swallows every key, so two of them are left alone or the keyboard
 * is trapped here: Escape, which cancels, and a bare Tab, which moves on to
 * « Enregistrer ». Blur is deliberately not a cancel -- pressing that button
 * blurs this one first and would discard the combination before the click lands.
 */
export function KeycapInput({
  combo,
  capturing = false,
  label,
  oncapture,
  oncancel,
  onstart,
}: {
  /** The combination to show, e.g. `alt+f10`. */
  combo: string;
  capturing?: boolean;
  label: string;
  oncapture: (e: KeyboardEvent) => void;
  /** Escape while capturing: drop the pending combination and show the saved one again. */
  oncancel: () => void;
  onstart: () => void;
}) {
  function onKeyDown(e: KeyboardEvent) {
    if (!capturing) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      oncancel();
      return;
    }
    if (e.key === 'Tab' && !e.ctrlKey && !e.altKey && !e.metaKey) return;
    oncapture(e);
  }

  const caps = formatCombo(combo);

  return (
    <button
      type="button"
      aria-label={label}
      onClick={onstart}
      onKeyDown={onKeyDown}
      className={`flex min-h-8 min-w-[172px] cursor-pointer items-center gap-1.5 rounded-lg border bg-surface px-2.5 py-1 text-sm text-default outline-none transition-[border-color,box-shadow] duration-200 ease-[cubic-bezier(0.22,1,0.36,1)] focus-visible:ring-2 focus-visible:ring-[var(--color-border-strong)] active:scale-[0.97] ${
        capturing ? 'border-[var(--color-border-strong)] ring-2 ring-[var(--color-border-strong)]/30' : 'border-default hover:border-strong'
      }`}
    >
      {caps.length === 0 ? (
        <span className="text-xs text-tertiary">Cliquer, puis appuyer sur les touches</span>
      ) : (
        caps.map((cap, i) => (
          <span key={i} className="inline-flex items-center gap-1.5">
            {i > 0 && <span className="text-xs text-tertiary">+</span>}
            <kbd className="rounded-md border border-strong bg-surface-tertiary px-1.5 py-0.5 font-sans text-xs font-semibold">{cap}</kbd>
          </span>
        ))
      )}
      {capturing && <span className="ml-1.5 text-xs whitespace-nowrap text-secondary">Échap pour annuler</span>}
    </button>
  );
}
