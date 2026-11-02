import { useRef, useState, type MouseEvent } from 'react';
import { Menu } from '@openai/apps-sdk-ui/components/Menu';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Checkbox } from '@openai/apps-sdk-ui/components/Checkbox';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { clipUrl, formatBytes, formatDuration, thumbUrl } from '../lib/clips';
import type { ClipMeta } from '../lib/types';
import { Copy, Dots, Folder, Pencil, Star, StarFilled, Trash } from './icons';

const EASE = 'ease-[cubic-bezier(0.22,1,0.36,1)]';

type ItemId = 'favorite' | 'rename' | 'copy' | 'reveal' | 'delete';

export function ClipCard({
  clip,
  clipDir,
  cacheDir = '',
  selected = false,
  selectable = false,
  checked = false,
  ontoggle,
  onselect,
  onfavorite,
  onrename,
  oncopy,
  onreveal,
  ondelete,
}: {
  clip: ClipMeta;
  clipDir: string;
  /** The library cache the thumbnail is read from; empty falls back to the video. */
  cacheDir?: string;
  selected?: boolean;
  selectable?: boolean;
  checked?: boolean;
  ontoggle?: (range: boolean) => void;
  onselect: (event?: MouseEvent) => void;
  onfavorite: () => void;
  onrename: (title: string) => void;
  oncopy: () => void;
  onreveal: () => void;
  ondelete: () => void;
}) {
  const [noThumb, setNoThumb] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  /** Where the card was right-clicked, while its right-click menu is open. */
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState('');
  const renamingRef = useRef(false);

  const startRename = () => {
    setDraft(clip.title);
    renamingRef.current = true;
    setRenaming(true);
  };
  const stopRename = () => {
    renamingRef.current = false;
    setRenaming(false);
  };

  const closeMenus = () => {
    setMenuOpen(false);
    setMenuAt(null);
  };

  const pick = (id: ItemId) => {
    closeMenus();
    if (id === 'favorite') onfavorite();
    else if (id === 'rename') startRename();
    else if (id === 'copy') oncopy();
    else if (id === 'reveal') onreveal();
    else ondelete();
  };

  const oncontextmenu = (e: MouseEvent) => {
    // The rename box keeps the webview's own Cut/Copy/Paste menu.
    if (renaming) return;
    e.preventDefault();
    onselect();
    setMenuOpen(false);
    setMenuAt({ x: e.clientX, y: e.clientY });
  };

  const commit = () => {
    if (!renamingRef.current) return;
    stopRename();
    const next = draft.trim();
    if (next && next !== clip.title) onrename(next);
  };

  /** One list for the `⋯` menu and the right-click menu, so they cannot drift apart. */
  const content = (
    <Menu.Content align={menuAt ? 'start' : 'end'} minWidth={200} width="auto">
      <Menu.Item onSelect={() => pick('favorite')}>
        <span className="flex items-center gap-2">
          {clip.favorite ? <StarFilled className="size-4" /> : <Star className="size-4" />}
          {clip.favorite ? 'Retirer des favoris' : 'Ajouter aux favoris'}
        </span>
      </Menu.Item>
      <Menu.Item onSelect={() => pick('rename')}>
        <span className="flex items-center gap-2">
          <Pencil className="size-4" />
          Renommer
        </span>
      </Menu.Item>
      <Menu.Item onSelect={() => pick('copy')}>
        <span className="flex items-center gap-2">
          <Copy className="size-4" />
          Copier le fichier
        </span>
      </Menu.Item>
      <Menu.Item onSelect={() => pick('reveal')}>
        <span className="flex items-center gap-2">
          <Folder className="size-4" />
          Afficher dans le dossier
        </span>
      </Menu.Item>
      <Menu.Separator />
      <Menu.Item onSelect={() => pick('delete')}>
        <span className="flex items-center gap-2 text-danger">
          <Trash className="size-4" />
          Supprimer
        </span>
      </Menu.Item>
    </Menu.Content>
  );

  const open = menuOpen || menuAt !== null;
  const ringShadow = checked
    ? 'shadow-[inset_0_0_0_3px_var(--color-ring),0_0_0_2px_var(--color-ring)]'
    : selected
      ? 'shadow-[inset_0_0_0_2px_var(--color-ring),0_0_0_3px_var(--color-background-primary-soft-alpha)]'
      : 'shadow-[inset_0_0_0_1px_var(--color-border-subtle)] group-hover:shadow-[inset_0_0_0_1px_var(--color-border-strong)]';

  // The right-click menu is anchored with `position: fixed` at the pointer, and
  // the hover lift's transform would turn this card into the box `fixed` is
  // measured from. So the lift (and its transition) is off while it is open.
  const lift = menuAt !== null ? '' : `transition-transform duration-200 ${EASE} hover:-translate-y-0.5`;

  return (
    <div
      className={`group relative grid min-w-0 gap-2 ${lift} ${open ? 'z-[1]' : ''}`}
      onContextMenu={oncontextmenu}>
      {selectable && (
        <span
          data-card-control
          className="absolute top-2 left-2 z-[2] rounded-md bg-surface/80 p-0.5"
          onClick={(event) => {
            event.stopPropagation();
            ontoggle?.(event.shiftKey);
          }}>
          <Checkbox checked={checked} {...{ 'aria-label': `Sélectionner ${clip.title}` }} />
        </span>
      )}
      {/* `aria-label`, because nothing inside this button can name it: the
          thumbnail is `alt=""` (it is the clip, not a description of it) and the
          only other child is the duration badge. */}
      <button
        type="button"
        className={`thumb relative aspect-[16/10] cursor-pointer overflow-hidden rounded-xl border-0 bg-surface-secondary p-0 transition-shadow duration-200 ${EASE} active:scale-[0.99] ${ringShadow}`}
        aria-label={clip.title}
        onClick={(event) => onselect(event)}>
        {cacheDir && !noThumb ? (
          <img
            className="block size-full object-cover"
            src={thumbUrl(cacheDir, clip)}
            alt=""
            loading="lazy"
            decoding="async"
            // No thumbnail (a still desktop sent no frame, or the cache was
            // cleared): the video's own frame takes its place.
            onError={() => setNoThumb(true)}
          />
        ) : (
          <video
            className="pointer-events-none block size-full object-cover"
            src={`${clipUrl(clipDir, clip)}#t=1`}
            preload="metadata"
            muted
            playsInline
            tabIndex={-1}
            aria-hidden="true"
            onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
          />
        )}
        <span className="absolute right-1.5 bottom-1.5 rounded-sm bg-black/70 px-1.5 py-px text-[10px] text-white tabular-nums">
          {formatDuration(clip.duration_ms)}
        </span>
      </button>

      <div className="grid gap-px px-0.5 pb-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          {/* The star lives here, not on the thumbnail: over a bright frame a
              bare glyph is invisible. `role="img"` with a name, because this is
              the only place kept state is expressed on a card. */}
          {clip.favorite && (
            <span className="flex flex-none text-warning" role="img" aria-label="Favori">
              <StarFilled className="size-3.5" aria-hidden="true" />
            </span>
          )}

          {renaming ? (
            <Input
              className="min-w-0 flex-1"
              size="2xs"
              data-card-control
              autoFocus
              aria-label={`Renommer ${clip.title}`}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={commit}
              onKeyDown={(e) => {
                // Stopped as well as handled: the grid listens on `window`, and
                // without this every keystroke typed here would also reach its
                // arrow/Enter/Space handling.
                e.stopPropagation();
                if (e.key === 'Enter') commit();
                else if (e.key === 'Escape') stopRename();
              }}
            />
          ) : (
            <>
              <span className="min-w-0 flex-1 truncate text-sm text-default">{clip.title}</span>
              <span
                data-card-control
                className={`flex-none transition-opacity duration-200 ${EASE} ${
                  open ? 'opacity-100' : 'opacity-0 group-focus-within:opacity-100 group-hover:opacity-100'
                } ${selected ? 'opacity-100' : ''}`}>
                <Menu forceOpen={menuOpen} onOpen={() => setMenuOpen(true)} onClose={closeMenus}>
                  <Menu.Trigger>
                    <Button
                      color="secondary"
                      variant="ghost"
                      size="2xs"
                      uniform
                      aria-label={`Autres actions pour ${clip.title}`}
                      aria-haspopup="menu">
                      <Dots />
                    </Button>
                  </Menu.Trigger>
                  {content}
                </Menu>
              </span>
            </>
          )}
        </div>
        <span className="truncate text-xs text-secondary" title={`${formatBytes(clip.bytes)} · ${clip.width}×${clip.height} · ${clip.fps} fps`}>
          {clip.game ? `${clip.game} · ` : ''}
          {Number.isFinite(Date.parse(clip.created)) ? (
            <time dateTime={clip.created}>{new Date(clip.created).toLocaleDateString('fr-FR', { day: 'numeric', month: 'short' })}</time>
          ) : 'Date inconnue'}
        </span>
      </div>

      {menuAt && (
        <Menu forceOpen onClose={closeMenus}>
          <Menu.Trigger>
            <span
              aria-hidden="true"
              className="pointer-events-none fixed size-0"
              style={{ left: menuAt.x, top: menuAt.y }}
            />
          </Menu.Trigger>
          {content}
        </Menu>
      )}
    </div>
  );
}
