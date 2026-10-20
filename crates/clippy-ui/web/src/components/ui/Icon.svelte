<script lang="ts">
  import { ICONS, isSolid, type IconName } from '../../lib/icons';

  /**
   * Every icon in this app is decorative: something outside it always carries
   * the name. Either it sits beside a text label, or it sits inside an
   * element that is named itself -- a control with an `aria-label`, or the
   * `role="img"` span `ClipCard` wraps its favourite marker in, which is the
   * one place an icon is the whole of the information. So this is
   * `aria-hidden` unconditionally and takes no title -- an icon that
   * announced itself would double up every button in the app.
   */
  let { name, size = 16 }: { name: IconName; size?: number } = $props();

  const def = $derived(ICONS[name]);
</script>

{#if isSolid(def)}
  <svg
    width={size}
    height={size}
    viewBox={def.viewBox}
    fill="currentColor"
    aria-hidden="true"
    focusable="false">
    {#each def.paths as path (path.d)}
      <path d={path.d} fill-rule={path.evenodd ? 'evenodd' : undefined} clip-rule={path.evenodd ? 'evenodd' : undefined} />
    {/each}
  </svg>
{:else}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false">
    <path d={def.d} />
  </svg>
{/if}

<style>
  svg { display: block; flex: 0 0 auto; }
</style>
