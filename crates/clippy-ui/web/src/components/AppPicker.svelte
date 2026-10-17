<script lang="ts">
  import { call } from '../lib/ipc';
  import { app } from '../lib/state.svelte';
  import { MAX_AUDIO_APPS, appLabel, withApp, withoutApp } from '../lib/settings';
  import type { AudioApp } from '../lib/types';
  import Button from './ui/Button.svelte';
  import Icon from './ui/Icon.svelte';
  import Menu, { type MenuItem } from './ui/Menu.svelte';

  /**
   * The applications whose sound is recorded, as the executable names the
   * daemon matches against, with a way to add one from what is making sound
   * right now or by typing its name.
   *
   * Empty means "everything the PC plays", and is said in words, because an
   * empty list of chips would otherwise read as "nothing is recorded".
   */
  let {
    selected,
    onchange,
    emptyText = 'Tout le son du PC',
    addText = 'Ajouter une application',
  }: {
    selected: string[];
    onchange: (next: string[]) => void;
    /** What an empty list means, in words. */
    emptyText?: string;
    addText?: string;
  } = $props();

  const OTHER = '\u0000other';

  let open = $state(false);
  let apps = $state<AudioApp[]>([]);
  let typing = $state(false);
  let typed = $state('');

  const items = $derived.by<MenuItem[]>(() => {
    const list: MenuItem[] = apps
      .filter((a) => !selected.includes(a.exe))
      .map((a) => ({ id: a.exe, label: a.playing ? `${a.name} · joue du son` : a.name }));
    list.push({ id: OTHER, label: 'Autre application…', separatorBefore: list.length > 0 ? true : undefined });
    return list;
  });

  async function show() {
    if (open) {
      open = false;
      return;
    }
    try {
      // Fetched on every opening: what is playing is the whole point of the list.
      apps = (await call<{ apps: AudioApp[] }>('audio.apps')).apps;
    } catch (e) {
      app.toast('error', String(e));
      apps = [];
    }
    open = true;
  }

  function pick(id: string) {
    open = false;
    if (id === OTHER) {
      typing = true;
      return;
    }
    onchange(withApp(selected, id));
  }

  function commitTyped() {
    const next = withApp(selected, typed);
    typing = false;
    typed = '';
    if (next !== selected) onchange(next);
  }

  function label(exe: string): string {
    return apps.find((a) => a.exe === exe)?.name ?? appLabel(exe);
  }
</script>

<div class="picker">
  {#if selected.length === 0}
    <span class="all">{emptyText}</span>
  {:else}
    {#each selected as exe (exe)}
      <span class="chip">
        {label(exe)}
        <button type="button" class="x" aria-label="Retirer {label(exe)}"
          onclick={() => onchange(withoutApp(selected, exe))}>
          <Icon name="close" size={9} />
        </button>
      </span>
    {/each}
  {/if}

  {#if typing}
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="name"
      type="text"
      autofocus
      placeholder="spotify.exe"
      aria-label="Nom de l’exécutable de l’application"
      bind:value={typed}
      onkeydown={(e) => {
        if (e.key === 'Enter') commitTyped();
        else if (e.key === 'Escape') { typing = false; typed = ''; }
      }}
      onblur={commitTyped} />
  {:else}
    <span class="add">
      <Button size="sm" disabled={selected.length >= MAX_AUDIO_APPS} onclick={show}>
        {selected.length >= MAX_AUDIO_APPS ? `${MAX_AUDIO_APPS} maximum` : addText}
      </Button>
      {#if open}
        <Menu {items} onpick={pick} onclose={() => (open = false)} />
      {/if}
    </span>
  {/if}
</div>

<style>
  .picker {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    max-width: 360px;
  }
  .all { color: var(--dim); font-size: 12.5px; margin-right: 4px; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px 4px 10px;
    background: var(--bg);
    border: 1px solid var(--line-strong);
    border-radius: var(--r);
    font-size: 12.5px;
  }
  .x {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    margin: -4px -2px -4px 0;
    padding: 0;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--dim);
    cursor: pointer;
  }
  .x:hover { background: var(--hover); color: var(--text); }
  .add { position: relative; }
  .name {
    width: 150px;
    padding: 6px 9px;
    background: var(--bg);
    border: 1px solid var(--line-strong);
    border-radius: var(--r);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
</style>
