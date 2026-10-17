<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { app } from '../lib/state.svelte';
  import IconButton from './ui/IconButton.svelte';
  import Logo from './Logo.svelte';

  const win = getCurrentWindow();
  let maximized = $state(false);

  // The maximize button has to show a restore glyph once the window is
  // maximized, and the window can be maximized without this button -- by
  // double-clicking the drag region, by Win+Up, by dragging to the top edge.
  // So the state is read from the window, never inferred from our own clicks.
  $effect(() => {
    let alive = true;
    void win.isMaximized().then((v) => { if (alive) maximized = v; });
    const un = win.onResized(() => {
      void win.isMaximized().then((v) => { if (alive) maximized = v; });
    });
    return () => {
      alive = false;
      void un.then((fn) => fn());
    };
  });

  const pct = $derived(
    app.status && app.status.ring_seconds_total > 0
      ? Math.min(100, (app.ringUsed / app.status.ring_seconds_total) * 100)
      : 0,
  );
</script>

<header class="tb">
  <Logo />
  <span class="name">CLIPPY</span>

  <button
    type="button"
    class="arm"
    class:on={app.armed}
    class:waiting={app.waiting && !app.armed}
    aria-label="Replay"
    aria-pressed={app.armed || app.waiting}
    title={app.waiting && !app.armed
      ? 'Le replay démarre dès qu’un jeu est au premier plan. Cliquer pour le désactiver.'
      : app.armed
        ? `${app.status?.game ? `Jeu détecté : ${app.status.game}. ` : ''}Cliquer pour désactiver le replay.`
        : undefined}
    disabled={!app.connected}
    onclick={() => app.toggleArm()}>
    <span class="dot"></span>
    {app.armed ? 'Replay actif' : app.waiting ? 'Replay prêt · attend un jeu' : 'Activer le replay'}
    {#if app.armed && app.status?.game}
      <span class="game">{app.status.game}</span>
    {/if}
    {#if app.armed}
      <span class="meter"><i style="transform: scaleX({pct / 100})"></i></span>
      <span class="n tnum">
        {app.ringUsed.toFixed(0)}/{app.status?.ring_seconds_total.toFixed(0) ?? '0'}&nbsp;s
      </span>
    {/if}
  </button>

  <!-- The whole remaining width drags the window. Tauri handles
       double-click-to-maximize on this attribute itself. -->
  <div class="grab" data-tauri-drag-region></div>

  <div class="wc">
    <IconButton icon="minimize" label="Réduire" size={11} onclick={() => void win.minimize()} />
    <IconButton
      icon={maximized ? 'restore' : 'maximize'}
      label={maximized ? 'Restaurer' : 'Agrandir'}
      size={11}
      onclick={() => void win.toggleMaximize()} />
    <span class="close">
      <IconButton icon="close" label="Fermer" size={11} onclick={() => void win.close()} />
    </span>
  </div>
</header>

<style>
  .tb {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 40px;
    padding-left: 12px;
    flex: 0 0 40px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  .name { font-size: 12px; font-weight: 600; letter-spacing: 0.05em; }
  .grab { flex: 1; align-self: stretch; }

  .arm {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 11px;
    border-radius: var(--r-full);
    border: 1px solid var(--line-strong);
    background: transparent;
    color: var(--dim);
    font: inherit;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    transition: color var(--t) var(--ease), background var(--t) var(--ease), border-color var(--t) var(--ease);
  }
  .arm .dot { width: 6px; height: 6px; border-radius: 50%; background: var(--faint); transition: background var(--t) var(--ease); }
  /* The pill is the most-pressed control in the app, so it answers the
     pointer in both of its states. Scoped off `.on` because the armed rule
     below is only (0,2,0) specificity and a bare `.arm:hover` would outrank
     it -- washing the green out from under the cursor. */
  .arm:hover:not(:disabled):not(.on) {
    background: var(--hover);
    border-color: var(--line-hi);
    color: var(--text);
  }
  .arm.on {
    background: color-mix(in srgb, var(--live) 13%, transparent);
    border-color: color-mix(in srgb, var(--live) 50%, transparent);
    color: var(--live);
  }
  .arm.on .dot { background: var(--live); box-shadow: 0 0 0 3px color-mix(in srgb, var(--live) 20%, transparent); }
  .arm.on:hover:not(:disabled) { background: color-mix(in srgb, var(--live) 20%, transparent); }
  /* On, but not recording: the dot keeps the live colour as an outline only, so
     it reads as "ready" and not as the green of a buffer that is filling. */
  .arm.waiting { color: var(--text); border-color: color-mix(in srgb, var(--live) 40%, var(--line-strong)); }
  .arm.waiting .dot { background: transparent; box-shadow: inset 0 0 0 1.5px var(--live); }
  .arm:disabled { opacity: 0.4; cursor: default; }
  .meter { width: 44px; height: 3px; border-radius: 2px; background: var(--line-strong); overflow: hidden; }
  .meter i { display: block; width: 100%; height: 100%; background: var(--live); transform-origin: left; transition: transform 200ms linear; }
  .n { opacity: 0.85; }
  .game {
    max-width: 16ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
    opacity: 0.85;
  }

  .wc { display: flex; align-self: stretch; }
  /* Windows-standard 44px, and square to the bar rather than the 30px
     rounded shape IconButton uses everywhere else. */
  .wc :global(.ib) { width: 44px; height: 100%; border-radius: 0; }
  .close :global(.ib:hover) { background: var(--win-close); color: var(--text); }
</style>
