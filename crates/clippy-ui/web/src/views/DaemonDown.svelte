<script lang="ts">
  import { startDaemon } from '../lib/ipc';
  import { app } from '../lib/state.svelte';
  import Button from '../components/ui/Button.svelte';

  let starting = $state(false);

  async function start() {
    starting = true;
    // The supervisor reconnects by itself and removes this panel when it does;
    // if that has not happened by now the service is not coming, and a button
    // stuck on "Démarrage…" would be a dead end.
    setTimeout(() => {
      if (!starting) return;
      starting = false;
      app.toast('error', 'Le service Clippy ne répond pas. Réessayer.');
    }, 15000);
    try {
      await startDaemon();
    } catch (e) {
      app.toast('error', String(e));
      starting = false;
    }
    // Deliberately not cleared on success: the supervisor reconnects on its
    // own and this whole panel disappears when it does.
  }
</script>

<div class="down">
  <h2>Clippy n’est pas lancé</h2>
  <p>Le service en arrière-plan gère le tampon de replay et le raccourci de clip. Rien n’est capturé pour le moment.</p>
  <Button variant="primary" disabled={starting} onclick={start}>
    {starting ? 'Démarrage…' : 'Démarrer Clippy'}
  </Button>
</div>

<style>
  .down {
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    flex: 1;
    text-align: center;
    padding: 40px;
  }
  h2 { margin: 0; font-size: 18px; font-weight: 650; }
  p { margin: 0 0 6px; max-width: 42ch; color: var(--dim); font-size: 12.5px; }
</style>
