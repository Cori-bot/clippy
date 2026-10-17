<script lang="ts">
  import { onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { call, onDaemonEvent } from '../lib/ipc';
  import { app, updates } from '../lib/state.svelte';
  import { FIELDS, SECTIONS, SECTION_LABELS, hotkeyProblem, inactiveReason, unknownKeys, validate } from '../lib/settings';
  import Field from '../components/Field.svelte';
  import Button from '../components/ui/Button.svelte';
  import Icon from '../components/ui/Icon.svelte';
  import type { Monitor } from '../lib/types';

  let config = $state<Record<string, unknown>>({});
  let monitors = $state<Monitor[]>([]);
  let extras = $state<string[]>([]);
  // `config` starts empty, and an empty config renders every toggle off and every
  // stepper at 0 -- values that are not the user's. The sections stay dimmed
  // until the daemon has actually answered.
  let loaded = $state(false);

  $effect(() => {
    void load();
  });

  // Unlike `wireDaemon()` in state.svelte.ts (which lives for the whole app
  // and is meant to), this handler belongs to a page the user opens and
  // leaves through the rail. Left registered, every visit would stack another
  // handler that outlives its component, still writing to a dead instance's
  // `config` -- so the returned unlisten function is captured and run when
  // this component is destroyed. (`Field.svelte` runs the same pattern for
  // its own `hotkey_pressed` listener, which is per-row rather than
  // page-level -- see its doc comment for why.)
  const unlisten = onDaemonEvent((event) => {
    if (event.event === 'hotkey_rebound') {
      if (event.data['registered'] === false) {
        app.toast('error', `Windows a refusé d’attribuer ${event.data['spec']} à Clippy. Une autre application l’utilise déjà.`);
      }
      // The toast is the alert; `status` is the record. Re-read it either way,
      // because a rebind that *succeeded* is what clears a warning left over
      // from the combination before it -- a page that only refreshed on
      // failure would keep telling a user their hotkey is taken after they
      // had already fixed it.
      void app.refreshStatus();
    }
    if (event.event === 'config_changed') {
      // The sound dialog and the tray change config behind this page's back;
      // without this the path box would keep showing the old file until the
      // page was reopened.
      config = { ...config, ...event.data };
    }
  });
  onDestroy(() => {
    void unlisten.then((fn) => fn());
  });

  async function load() {
    try {
      // Before the fields render, because a hotkey row's warning is read off
      // `status` and this page can be opened long after the connect that last
      // fetched one.
      await app.refreshStatus();
      config = await call<Record<string, unknown>>('config.get');
      loaded = true;
      extras = unknownKeys(config);
      const list = await call<{ monitors: Monitor[] }>('monitors.list');
      monitors = list.monitors;
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  async function set(key: string, value: unknown) {
    const complaint = validate(key, value);
    if (complaint) {
      app.toast('error', complaint);
      return;
    }
    try {
      const result = await call<{ accepted: Record<string, unknown>; requires_rearm: string[] }>(
        'config.set',
        { [key]: value },
      );
      // Read back from `accepted`, which the daemon takes out of the saved
      // config rather than echoing the request: an accepted value that differs
      // from what was typed is exactly what the field should now show.
      config = { ...config, ...result.accepted };
      if (result.requires_rearm.length > 0 && app.armed) {
        app.addRearmNeeded(result.requires_rearm);
      }
    } catch (e) {
      app.toast('error', String(e));
      // The daemon refused, so nothing was written and nothing was applied:
      // put the field back to the truth rather than leaving the typed value on
      // screen looking saved.
      await load();
    }
  }

  async function pickFolder() {
    try {
      // Same contract as `pickSound` below: this answers as soon as the dialog
      // is open, and the chosen folder arrives as a `config_changed` event.
      // The daemon reports a folder it cannot use in its own message box, so
      // there is nothing to toast here beyond the dialog failing to open.
      await call('folder.pick');
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  async function pickSound() {
    try {
      // Answers as soon as the dialog is open, not when it closes. The chosen
      // file arrives as a `config_changed` event, handled below.
      await call('sound.pick');
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  async function testSound() {
    try {
      await call('sound.test');
    } catch (e) {
      app.toast('error', String(e));
    }
  }

  function jump(section: string) {
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    document.getElementById(`section-${section}`)?.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' });
  }

  let version = $state('');
  let checking = $state(false);
  let checked = $state<string | null>(null);

  invoke<string>('update_current_version').then((v) => (version = v));

  /**
   * Unlike the check on launch, this one reports either way -- the user asked,
   * so silence would read as a broken button.
   *
   * Renders from `update_check`'s return value, never from the `clippy-update`
   * channel: that channel stays deliberately silent while an install is in
   * flight (see `update/mod.rs`'s `emit_check`), so a channel-driven button
   * would go dead with no feedback the moment it mattered most.
   */
  async function checkNow() {
    checking = true;
    checked = null;
    try {
      const found = await invoke<{ version: string } | null>('update_check');
      checked = found ? `Clippy ${found.version} est disponible.` : 'Clippy est à jour.';
    } catch (e) {
      // Rust already writes a complete, correctly-attributed sentence here
      // (`reported()`'s job) -- a network failure, a 403 from being
      // rate-limited, or a malformed feed each get their own wording, and
      // none of them start with "could not reach GitHub" except the one that
      // actually is that. Render it unprefixed, the same way App.svelte
      // renders `banner.error`; prefixing it here used to double up the
      // network case ("Could not reach GitHub: could not reach GitHub: ...")
      // and mislabel the other two as network failures they were not.
      checked = String(e);
    } finally {
      checking = false;
    }
  }
</script>

<h1>Paramètres</h1>
<p class="lede">Les modifications s’appliquent au prochain clip. Certaines demandent de réactiver le replay, et Clippy indique lesquelles.</p>

<nav class="jump" aria-label="Sections des paramètres">
  {#each SECTIONS as section (section)}
    <button type="button" onclick={() => jump(section)}>{SECTION_LABELS[section]}</button>
  {/each}
</nav>

{#if app.rearmNeeded.length > 0}
  <!-- `--live`, not `--accent`: this is about the running capture, which is
       what green means everywhere else in the app. -->
  <div class="notice">
    <Icon name="rearm" size={15} />
    Réactiver le replay pour appliquer&nbsp;: {app.rearmNeeded.map((k) => FIELDS.find((f) => f.key === k)?.label ?? k).join(', ')}
    <span class="spacer"></span>
    <Button size="sm" onclick={() => app.rearmNow()}>Réactiver maintenant</Button>
  </div>
{/if}

{#snippet row(field: (typeof FIELDS)[number])}
  <Field
    {field} {config} {monitors}
    problem={hotkeyProblem(field, field.boundKey ? app.status?.[field.boundKey] : null, config)}
    inactive={inactiveReason(field, config)}
    onset={set}
    onpickfolder={pickFolder}
    onpicksound={pickSound}
    ontestsound={testSound} />
{/snippet}

{#each SECTIONS as section (section)}
  {@const fields = FIELDS.filter((f) => f.section === section)}
  <section id="section-{section}" aria-busy={!loaded} class:loading={!loaded}>
    <h2>{SECTION_LABELS[section]}</h2>
    <div class="panel">
      {#each fields.filter((f) => !f.advanced) as field (field.key)}
        {@render row(field)}
      {/each}
      {#if fields.some((f) => f.advanced)}
        <details class="adv">
          <summary>Avancé</summary>
          {#each fields.filter((f) => f.advanced) as field (field.key)}
            {@render row(field)}
          {/each}
        </details>
      {/if}
      {#if section === 'Updates'}
        <div class="row">
          <div class="lt">
            <b>Version</b>
            <span>Version de Clippy actuellement utilisée.</span>
            {#if checked}<span class="checked">{checked}</span>{/if}
          </div>
          <div class="rt">
            <span class="ver tnum">{version}</span>
            <Button size="sm" disabled={checking || updates.busy} onclick={checkNow}>
              {checking ? 'Vérification…' : 'Vérifier maintenant'}
            </Button>
          </div>
        </div>
      {/if}
    </div>
  </section>
{/each}

{#if extras.length > 0}
  <p class="extras">
    Le service Clippy expose des paramètres que cette application n’affiche pas encore&nbsp;: {extras.join(', ')}. Ils se modifient dans config.toml.
  </p>
{/if}

<style>
  h1 { font-size: 19px; font-weight: 650; margin: 0 0 4px; }
  .lede { color: var(--dim); font-size: 12px; margin: 0 0 12px; }
  /* Sticky at the top of the scrollport, with the re-arm notice parked just
     under it: the page is three screens long and the headings are its only
     landmarks. Opaque, for the same reason the notice is. */
  .jump {
    position: sticky;
    top: 0;
    z-index: 4;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    height: 36px;
    box-sizing: border-box;
    align-items: center;
    margin-bottom: 14px;
    background: var(--bg);
    border-bottom: 1px solid var(--line-soft);
  }
  .jump button {
    padding: 4px 9px;
    border: 0;
    border-radius: var(--r-full);
    background: transparent;
    color: var(--dim);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    transition: background var(--t-fast) var(--ease), color var(--t-fast) var(--ease);
  }
  .jump button:hover { background: var(--hover); color: var(--text); }
  section { margin-bottom: 20px; scroll-margin-top: 48px; transition: opacity var(--t) var(--ease); }
  section.loading { opacity: 0.5; }
  h2 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--dim);
    font-weight: 600;
    margin: 0 0 8px;
  }
  .adv { border-top: 1px solid var(--line-soft); }
  .adv summary {
    padding: 11px 0;
    color: var(--dim);
    font-size: 12px;
    cursor: pointer;
  }
  .adv summary:hover { color: var(--text); }
  .panel {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--r-lg);
    padding: 2px 15px;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 10px 13px;
    margin-bottom: 18px;
    border-radius: var(--r-md);
    font-size: 12.5px;
    /* Sticky, not static. The settings list is longer than the window, and
       the keys that need a re-arm are scattered down it -- so a notice at the
       top of the document was, in practice, a notice you had to scroll back
       up to find after changing the thing that raised it. `top: 0` is the top
       of `.content`'s scrollport (App.svelte), which sits inside that
       element's padding, so this ends flush with the top of the visible area
       with nothing peeking above it. Nothing between here and `.content`
       sets `transform`, `filter` or `contain`, any of which would silently
       turn this back into a static box. */
    position: sticky;
    top: 36px;
    z-index: 3;
    /* Opaque, over `--bg` rather than `transparent`: the page scrolls
       underneath this now, and a translucent banner would show the heading
       sliding through it. */
    background: color-mix(in srgb, var(--live) 10%, var(--bg));
    border: 1px solid color-mix(in srgb, var(--live) 40%, transparent);
    box-shadow: 0 8px 16px -12px rgba(0, 0, 0, 0.9);
  }
  .notice :global(svg) { color: var(--live); }
  .spacer { margin-left: auto; }
  /* `.row`, `.lt` and `.rt` come from app.css, shared with `Field.svelte` --
     the Version row is this same shape and must line up with the generated
     rows above it. Only what is unique to this row lives here. */
  .checked { color: var(--text) !important; }
  .ver { color: var(--dim); font-size: 12.5px; }
  .extras { font-size: 11.5px; color: var(--dim); margin: 0; }
</style>
