<script lang="ts">
  import { formatCombo } from '../../lib/ui';

  let {
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
  } = $props();

  // Capturing swallows every key, so two of them have to be left alone or the
  // keyboard is trapped here: Escape, which cancels, and a bare Tab, which
  // moves on to "Enregistrer". Blur is deliberately not a cancel -- pressing
  // that button blurs this one first and would discard the combination before
  // the click lands.
  function onkey(e: KeyboardEvent) {
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

  const caps = $derived(formatCombo(combo));
</script>

<!--
  Always a button. There used to be a `readonly` box shell as well, for the
  setup wizard's confirm step, back when that step could show a hotkey but not
  change one -- which is precisely the dead end that made a user whose Alt+F10
  was taken unable to finish setting Clippy up. The wizard records like any other
  row now, so the second shell went with the bug.
-->
<button
  type="button"
  class="hk"
  class:capturing
  aria-label={label}
  onclick={onstart}
  onkeydown={onkey}>
  {#if caps.length === 0}
    <span class="ask">Cliquer, puis appuyer sur les touches</span>
  {:else}
    {#each caps as cap, i (i)}
      {#if i > 0}<span class="plus">+</span>{/if}
      <kbd>{cap}</kbd>
    {/each}
  {/if}
  {#if capturing}<span class="esc">Échap pour annuler</span>{/if}
</button>

<style>
  .hk {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 172px;
    padding: 6px 10px;
    background: var(--bg);
    border: 1px solid var(--field-line);
    border-radius: var(--r);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .hk.capturing { border-color: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent); }
  kbd {
    padding: 2px 7px;
    border-radius: var(--r-sm);
    background: var(--raised);
    border: 1px solid var(--line-strong);
    border-bottom-width: 2px;
    font: inherit;
    font-size: 11px;
    font-weight: 600;
  }
  .plus { color: var(--faint); font-size: 11px; }
  .ask { color: var(--faint); font-size: 12px; }
  .esc { margin-left: 6px; color: var(--dim); font-size: 10.5px; white-space: nowrap; }
</style>
