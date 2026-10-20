import { useState, type MouseEvent } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Menu } from '@openai/apps-sdk-ui/components/Menu';
import { app, useApp } from '../lib/app';
import { formatShotDate, shotThumbUrl, shotUrl } from '../lib/shots';
import { formatBytes } from '../lib/clips';
import type { ShotMeta } from '../lib/types';
import { Copy, Folder, Trash } from './icons';

/**
 * One screenshot tile. `onDelete` only asks: `Shots` hosts the confirmation.
 * The right-click menu offers the same three actions as the tile's buttons.
 */
export function ShotCard({
  shot,
  selected,
  onOpen,
  onSelect,
  onDelete,
}: {
  shot: ShotMeta;
  selected: boolean;
  onOpen: () => void;
  onSelect: () => void;
  onDelete: () => void;
}) {
  const a = useApp();
  const dir = a.status?.clip_dir ?? '';
  const taken = formatShotDate(shot.created);
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  // The thumbnail, falling back to the full image when its encode failed.
  const [full, setFull] = useState(false);
  const src = !dir ? '' : full ? shotUrl(dir, shot.id) : shotThumbUrl(dir, shot.id);

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    onSelect();
    setMenuAt({ x: e.clientX, y: e.clientY });
  }

  const pill =
    'size-7 text-white transition-[transform,background-color,color] duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97] hover:bg-white/15';

  return (
    // The right-click is a pointer shortcut to the tile's own buttons, not the
    // only way to reach them, so the card needs no role for it.
    <div
      onContextMenu={onContextMenu}
      className={`group relative overflow-hidden rounded-xl border bg-surface transition-colors duration-150 ${
        selected ? 'border-strong ring-1 ring-[var(--color-border-strong)]' : 'border-subtle hover:border-default'
      }`}>
      <button
        type="button"
        onClick={onOpen}
        aria-label={`Ouvrir la capture d’écran du ${taken}`}
        className="block w-full cursor-pointer border-0 bg-transparent p-0 outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-border-strong)]">
        <img
          src={src}
          alt=""
          loading="lazy"
          onError={() => {
            if (dir) setFull(true);
          }}
          className="block aspect-[16/10] w-full bg-surface-secondary object-cover"
        />
      </button>
      <div className="flex flex-col gap-0.5 px-3 py-2">
        <span className="text-sm text-default">{taken}</span>
        <span className="text-xs text-tertiary tabular-nums">
          {shot.width}x{shot.height} · {formatBytes(shot.bytes)}
        </span>
      </div>
      {/* Marker read by the grid's key handler (`CARD_CONTROL_SELECTOR`): keeps
          Enter and Space on these buttons. A dark pill, since they sit on a
          screenshot that can be any colour. */}
      <div
        data-card-control
        className={`absolute right-1.5 top-1.5 flex gap-0.5 rounded-xl border border-white/15 bg-black/70 p-[3px] shadow-lg transition-opacity duration-200 ease-[cubic-bezier(0.22,1,0.36,1)] group-hover:opacity-100 group-focus-within:opacity-100 ${
          selected ? 'opacity-100' : 'opacity-0'
        }`}>
        <Button
          color="secondary"
          variant="ghost"
          size="xs"
          uniform
          aria-label="Copier"
          title="Copier"
          className={pill}
          onClick={() => app.copyShot(shot.id)}>
          <Copy />
        </Button>
        <Button
          color="secondary"
          variant="ghost"
          size="xs"
          uniform
          aria-label="Afficher dans le dossier"
          title="Afficher dans le dossier"
          className={pill}
          onClick={() => app.revealShot(shot.id)}>
          <Folder />
        </Button>
        <Button
          color="secondary"
          variant="ghost"
          size="xs"
          uniform
          aria-label="Supprimer"
          title="Supprimer"
          className={`${pill} hover:!bg-[var(--color-background-danger-soft-alpha)] hover:text-danger`}
          onClick={onDelete}>
          <Trash />
        </Button>
      </div>

      {menuAt && (
        <Menu forceOpen onClose={() => setMenuAt(null)}>
          <Menu.Trigger>
            <span aria-hidden className="fixed size-0" style={{ left: menuAt.x, top: menuAt.y }} />
          </Menu.Trigger>
          <Menu.Content side="bottom" align="start" sideOffset={0} minWidth={200}>
            <Menu.Item onSelect={() => app.copyShot(shot.id)}>
              <Copy className="size-4" /> Copier
            </Menu.Item>
            <Menu.Item onSelect={() => app.revealShot(shot.id)}>
              <Folder className="size-4" /> Afficher dans le dossier
            </Menu.Item>
            <Menu.Separator />
            <Menu.Item onSelect={onDelete} className="text-danger">
              <Trash className="size-4" /> Supprimer
            </Menu.Item>
          </Menu.Content>
        </Menu>
      )}
    </div>
  );
}
