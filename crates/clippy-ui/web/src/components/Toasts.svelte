<script lang="ts">
  import { app } from '../lib/state.svelte';
  import Icon from './ui/Icon.svelte';
  import IconButton from './ui/IconButton.svelte';
</script>

<div class="toasts">
  {#each app.toasts as toast (toast.id)}
    <div class="toast" class:error={toast.kind === 'error'} role={toast.kind === 'error' ? 'alert' : 'status'}>
      <Icon name={toast.kind === 'error' ? 'alert' : 'check'} size={14} />
      <span>{toast.text}</span>
      <IconButton icon="close" label="Fermer la notification" size={11} onclick={() => app.dismissToast(toast.id)} />
    </div>
  {/each}
</div>

<style>
  .toasts { position: fixed; right: 18px; bottom: 18px; display: grid; gap: 8px; z-index: 50; }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    padding: 10px 8px 10px 14px;
    max-width: 46ch;
    border-radius: var(--r-md);
    background: var(--overlay);
    border: 1px solid var(--line-strong);
    box-shadow: var(--shadow);
    font-size: 12.5px;
    animation: slide var(--t-slow) var(--ease);
  }
  .toast span { flex: 1; min-width: 0; padding-top: 1px; }
  .toast > :global(svg) { flex: none; margin-top: 2px; color: var(--live); }
  .toast :global(.ib) { flex: none; width: 24px; height: 24px; margin-top: -2px; }
  .toast.error { border-color: color-mix(in srgb, var(--danger) 45%, var(--line-strong)); }
  .toast.error > :global(svg) { color: var(--danger); }
  /* Rises from below with a soft cross-blur: slower in than out would be, and never from nothing. */
  @keyframes slide { from { opacity: 0; transform: translateY(var(--distance-base)); filter: blur(var(--blur-small)); } }
</style>
