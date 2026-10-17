<script lang="ts">
  import { app } from '../lib/state.svelte';
  import Icon from '../components/ui/Icon.svelte';
</script>

<nav class="rail" aria-label="Navigation">
  <button type="button" class="nav" class:on={app.view === 'grid'} aria-current={app.view === 'grid' ? 'page' : undefined} onclick={() => (app.view = 'grid')}>
    <Icon name="clips" size={15} />Clips
  </button>
  <button type="button" class="nav" class:on={app.view === 'shots'} aria-current={app.view === 'shots' ? 'page' : undefined} onclick={() => (app.view = 'shots')}>
    <Icon name="camera" size={15} />Captures d’écran
  </button>
  <button type="button" class="nav" class:on={app.view === 'settings'} aria-current={app.view === 'settings' ? 'page' : undefined} onclick={() => (app.view = 'settings')}>
    <Icon name="settings" size={15} />Paramètres
  </button>
  <div class="ver tnum">{app.status?.version ?? ''}</div>
</nav>

<style>
  .rail {
    width: 188px;
    flex: 0 0 188px;
    padding: 12px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 10px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--dim);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--t-fast) var(--ease), color var(--t-fast) var(--ease);
  }
  .nav:hover { background: var(--hover); color: var(--text); }
  .nav.on { background: color-mix(in srgb, var(--accent) 15%, transparent); color: var(--text); font-weight: 600; }
  .ver { margin-top: auto; font-size: 10px; color: var(--faint); padding: 0 10px; }
</style>
