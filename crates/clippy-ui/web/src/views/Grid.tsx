import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { flushSync } from 'react-dom';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { EmptyMessage } from '@openai/apps-sdk-ui/components/EmptyMessage';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { Select } from '@openai/apps-sdk-ui/components/Select';
import { ClipCard } from '../components/ClipCard';
import { Dialog } from '../components/ui/Dialog';
import { Check, Close, Search, Star, StarFilled, Trash } from '../components/icons';
import type { ClipSort } from '../lib/library-query';
import { app, useApp } from '../lib/app';
import type { ClipMeta } from '../lib/types';
import { shouldHandleKey, moveSelection, CARD_CONTROL_SELECTOR } from '../lib/keys';
import { clipUrl, formatBytes } from '../lib/clips';
import { gridWindow, toggleClipSelection } from '../lib/clip-grid';
import { formatCombo } from '../lib/ui';

export function Grid() {
  const a = useApp();
  const hotkeyCaps = formatCombo(a.hotkey);
  const [columns, setColumns] = useState(4);
  const [stride, setStride] = useState(240);
  const [scroll, setScroll] = useState(0);
  const [viewport, setViewport] = useState(800);
  const [gridEl, setGridEl] = useState<HTMLDivElement | null>(null);
  const [previewing, setPreviewing] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<ClipMeta[] | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [anchor, setAnchor] = useState<string | null>(null);

  const scrollerRef = useRef<HTMLElement | null>(null);
  const deleteContext = useRef(0);
  const checkedRef = useRef(checked);
  checkedRef.current = checked;
  const anchorRef = useRef(anchor);
  anchorRef.current = anchor;
  const metrics = useRef({ columns, stride });
  metrics.current = { columns, stride };

  const visible = a.visible;
  const games = useMemo(() => [...new Set(a.clips.map(clip => clip.game).filter((game): game is string => !!game))]
    .sort((x, y) => x.localeCompare(y, 'fr')), [a.clips]);
  const filtered = !!a.searchQuery.trim() || a.gameFilter !== null || a.favoritesOnly;
  const windowed = gridWindow(visible.length, columns, stride, scroll, viewport);
  const mounted = visible.slice(windowed.start, windowed.end);
  const selectedClips = visible.filter((clip) => checked.has(clip.id));
  const selectedBytes = selectedClips.reduce((sum, clip) => sum + clip.bytes, 0);
  const busy = a.deleteProgress !== null;
  const canSelect = !a.clipsLoading && !a.clipsLoadError && !busy;

  const clearSelection = () => {
    setSelecting(false);
    setChecked(new Set());
    setAnchor(null);
  };

  const changeCriteria = (change: () => void) => {
    clearSelection();
    setPreviewing(null);
    change();
    scrollerRef.current?.scrollTo({ top: 0 });
    requestAnimationFrame(measure);
  };

  const toggleCheck = (id: string, range = false) => {
    if (!canSelect) return;
    setSelecting(true);
    const next = toggleClipSelection(checkedRef.current, app.visible.map((c) => c.id), id, anchorRef.current, range);
    checkedRef.current = next;
    setChecked(next);
    if (!range || anchorRef.current === null) {
      anchorRef.current = id;
      setAnchor(id);
    }
    app.selected = Math.max(0, app.visible.findIndex((c) => c.id === id));
    setPreviewing(null);
  };

  const selectAll = () => {
    if (!canSelect) return;
    setSelecting(true);
    setChecked(new Set(app.visible.map((c) => c.id)));
    setAnchor(app.visible[0]?.id ?? null);
    setPreviewing(null);
  };

  const pick = (clip: ClipMeta, index: number, event?: MouseEvent) => {
    app.selected = index;
    if (event && (selecting || event.ctrlKey || event.metaKey || event.shiftKey)) {
      toggleCheck(clip.id, event.shiftKey);
    } else if (event && !selecting && !busy) {
      // A click (or Enter) opens; a right click selects without an event and
      // must leave the grid, and its menu, where they are.
      app.view = 'clip';
    }
  };

  const askDelete = (clips: ClipMeta[]) => {
    if (app.deleteProgress !== null || clips.length === 0) return;
    setPreviewing(null);
    deleteContext.current = app.libraryContext;
    setDeleting(clips.slice());
  };

  const confirmDelete = async () => {
    if (!deleting || busy) return;
    const ids = deleting.map((clip) => clip.id);
    const context = deleteContext.current;
    const failed = await app.removeMany(ids, true, context);
    setDeleting(null);
    if (context !== app.libraryContext) return;
    const attempted = new Set(ids);
    const next = new Set([...checkedRef.current].filter((id) => !attempted.has(id)).concat(failed));
    if (next.size === 0) clearSelection();
    else setChecked(next);
  };

  // A folder switch must not leave a confirmation targeting ids from the old
  // folder. Scrolling and live prepends, in contrast, keep selection by id.
  useEffect(() => {
    if (app.deleteProgress === null) setDeleting(null);
    setSelecting(false);
    setChecked(new Set());
    setAnchor(null);
  }, [a.clipDir, a.libraryContext]);

  const measure = useCallback(() => {
    const scroller = scrollerRef.current;
    if (!gridEl || !scroller) return;
    const style = getComputedStyle(gridEl);
    setColumns(Math.max(1, style.gridTemplateColumns.split(' ').length));
    const tile = gridEl.firstElementChild;
    if (tile instanceof HTMLElement) setStride(tile.offsetHeight + parseFloat(style.rowGap));
    setViewport(scroller.clientHeight);
    setScroll(Math.max(0, scroller.getBoundingClientRect().top - gridEl.getBoundingClientRect().top));
  }, [gridEl]);

  useLayoutEffect(() => {
    measure();
  }, [measure, selecting, a.clipsLoading, a.clipsLoadError]);

  const revealSelected = useCallback(
    (focus = false) => {
      const scroller = scrollerRef.current;
      if (!gridEl || !scroller) return;
      // The selected tile may not exist in the DOM yet. Scroll using its row,
      // then let React mount that row before restoring keyboard focus.
      const origin = gridEl.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
      const { columns: cols, stride: step } = metrics.current;
      const top = origin + Math.floor(app.selected / cols) * step;
      const bottom = top + step;
      if (app.selected < cols) scroller.scrollTop = 0;
      else if (top < scroller.scrollTop) scroller.scrollTop = Math.max(0, top - 14);
      else if (bottom > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = bottom - scroller.clientHeight;
      flushSync(measure);
      if (focus) {
        gridEl
          .querySelector<HTMLElement>('[data-index="' + app.selected + '"] .thumb')
          ?.focus({ preventScroll: true });
      }
    },
    [gridEl, measure],
  );

  // The scroll container belongs to App, so subscribe to it rather than the
  // window. Only the visible rows plus two spare rows on each side are mounted.
  useEffect(() => {
    if (!gridEl) return;
    const host = gridEl.closest('main');
    if (!host) return;
    scrollerRef.current = host;
    let frame = 0;
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    host.addEventListener('scroll', schedule, { passive: true });
    const observer = new ResizeObserver(schedule);
    observer.observe(host);
    observer.observe(gridEl);
    measure();
    const reveal = requestAnimationFrame(() => revealSelected());
    return () => {
      host.removeEventListener('scroll', schedule);
      observer.disconnect();
      cancelAnimationFrame(frame);
      cancelAnimationFrame(reveal);
      scrollerRef.current = null;
    };
  }, [gridEl, measure, revealSelected]);

  const onKeyDown = (event: KeyboardEvent) => {
    if (deleting || app.deleteProgress !== null || event.defaultPrevented) return;
    const clips = app.visible;
    const target = event.target instanceof Element ? event.target : null;
    const ownedByCardControl = !!target?.closest(CARD_CONTROL_SELECTOR);
    const insideGrid = !ownedByCardControl && !!gridEl && event.target instanceof Node && gridEl.contains(event.target);
    if (!shouldHandleKey(event.target, event.key, insideGrid)) return;
    if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'c' && !selecting) {
      const clip = clips[app.selected];
      // A focused menu belongs to its own card, maybe not the selected one;
      // AltGr (Ctrl+Alt) and Ctrl+Shift+C are other keys, and a held key
      // must not copy again and again.
      if (clip && !event.repeat && !target?.closest('[role="menu"],[role="dialog"]') && !window.getSelection()?.toString()) {
        event.preventDefault();
        void app.copyClip(clip.id);
      }
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
      event.preventDefault();
      selectAll();
      return;
    }
    if (event.key === 'Escape') {
      setPreviewing(null);
      clearSelection();
      return;
    }
    if (event.key === 'Enter') {
      if (clips.length === 0 || selecting) return;
      event.preventDefault();
      app.view = 'clip';
      return;
    }
    if (event.key === ' ') {
      event.preventDefault();
      const clip = clips[app.selected];
      if (selecting && clip) toggleCheck(clip.id, event.shiftKey);
      else setPreviewing(clip && previewing !== clip.id ? clip.id : null);
      return;
    }
    if (event.key === 'Delete') {
      event.preventDefault();
      if (selecting) askDelete(selectedClips);
      else {
        const clip = clips[app.selected];
        if (clip) askDelete([clip]);
      }
      return;
    }
    const next = moveSelection(app.selected, event.key, clips.length, metrics.current.columns);
    if (next !== app.selected) {
      event.preventDefault();
      const previous = clips[app.selected]?.id;
      app.selected = next;
      setPreviewing(null);
      if (event.shiftKey && canSelect) {
        if (anchorRef.current === null) {
          anchorRef.current = previous ?? null;
          setAnchor(anchorRef.current);
        }
        toggleCheck(clips[next].id, true);
      }
      revealSelected(insideGrid);
    }
  };
  const keyHandler = useRef(onKeyDown);
  keyHandler.current = onKeyDown;
  useEffect(() => {
    const listener = (event: KeyboardEvent) => keyHandler.current(event);
    window.addEventListener('keydown', listener);
    return () => window.removeEventListener('keydown', listener);
  }, []);

  const count = a.total > 1 ? 'clips' : 'clip';
  const selectedCount = selectedClips.length;

  return (
    <>
      <header className="mb-5 flex flex-wrap items-center gap-2.5">
        <h1 className="heading-lg m-0">Clips</h1>
        <span className="text-xs text-tertiary tabular-nums">
          {filtered ? `${visible.length} sur ` : ''}
          {a.total} {count}
          {a.totalBytes !== null ? ` · ${formatBytes(a.totalBytes)}` : ''}
        </span>
        {a.clips.length > 0 && (
          <div className="ml-auto flex items-center gap-1.5">
            <Button
              color={selecting ? 'primary' : 'secondary'}
              variant={selecting ? 'soft' : 'outline'}
              size="sm"
              disabled={!canSelect}
              aria-pressed={selecting}
              onClick={() => (selecting ? clearSelection() : setSelecting(true))}>
              <Check />
              {selecting ? 'Terminé' : 'Sélectionner'}
            </Button>
            <Button
              color={a.favoritesOnly ? 'warning' : 'secondary'}
              className="favorite-control"
              variant={a.favoritesOnly ? 'soft' : 'outline'}
              size="sm"
              disabled={busy}
              aria-pressed={a.favoritesOnly}
              onClick={() => {
                changeCriteria(() => app.setFavoritesOnly(!app.favoritesOnly));
              }}>
              {a.favoritesOnly ? <StarFilled /> : <Star />}
              Favoris
            </Button>
          </div>
        )}
      </header>

      {a.clips.length > 0 && (
        <div className="mb-5 flex flex-wrap items-center gap-2" role="search" aria-label="Rechercher dans les clips"
          onKeyDown={event => event.stopPropagation()}>
          <Input
            className="search-control min-w-[200px] flex-1"
            type="search"
            size="lg"
            variant="soft"
            aria-label="Rechercher un clip ou un jeu"
            placeholder="Rechercher un clip ou un jeu"
            value={a.searchQuery}
            disabled={busy}
            startAdornment={<Search className="size-4 text-secondary" />}
            onChange={event => changeCriteria(() => app.setSearchQuery(event.target.value))}
          />
          <Select
            aria-label="Filtrer par jeu"
            size="lg"
            variant="soft"
            block={false}
            disabled={busy}
            value={a.gameFilter ?? ''}
            options={[{ value: '', label: 'Tous les jeux' }, ...games.map(game => ({ value: game, label: game }))]}
            onChange={option => changeCriteria(() => app.setGameFilter(option.value || null))}
          />
          <Select
            aria-label="Trier les clips"
            size="lg"
            variant="soft"
            block={false}
            disabled={busy}
            value={a.sortOrder}
            options={[
              { value: 'newest', label: 'Plus récents' },
              { value: 'oldest', label: 'Plus anciens' },
              { value: 'duration', label: 'Plus longs' },
              { value: 'size', label: 'Plus volumineux' },
            ]}
            onChange={option => changeCriteria(() => app.setSortOrder(option.value as ClipSort))}
          />
          {filtered && (
            <Button size="sm" color="secondary" variant="ghost" disabled={busy}
              onClick={() => changeCriteria(() => app.resetLibraryFilters())}>
              <Close /> Effacer les filtres
            </Button>
          )}
        </div>
      )}

      {selecting && (
        <div
          className="sticky -top-6 z-[5] mb-3.5 flex flex-wrap items-center gap-2 rounded-xl border border-default bg-surface-elevated p-2.5 text-sm"
          role="region"
          aria-label="Sélection de clips">
          <span className="mr-auto tabular-nums" aria-live="polite">
            {selectedCount} {selectedCount > 1 ? 'sélectionnés' : 'sélectionné'} · {formatBytes(selectedBytes)}
          </span>
          <Button
            size="sm"
            color="secondary"
            variant="ghost"
            disabled={!canSelect || selectedCount === visible.length}
            onClick={selectAll}>
            {a.favoritesOnly ? 'Sélectionner tous les favoris' : 'Tout sélectionner'}
          </Button>
          <Button
            size="sm"
            color="secondary"
            variant="ghost"
            disabled={busy || checked.size === 0}
            onClick={() => {
              setChecked(new Set());
              setAnchor(null);
            }}>
            Désélectionner
          </Button>
          <Button
            size="sm"
            color="danger"
            variant="soft"
            disabled={busy || selectedCount === 0}
            onClick={() => askDelete(selectedClips)}>
            <Trash />
            Supprimer la sélection ({selectedCount})
          </Button>
        </div>
      )}

      {a.clipsLoading ? (
        <p className="mb-3.5 text-sm text-secondary" role="status">
          Chargement des clips…
        </p>
      ) : a.clipsLoadError ? (
        <div className="mb-3.5 flex items-center gap-2.5 text-sm text-secondary" role="status">
          <span>Certains clips n’ont pas pu être chargés. {a.clipsLoadError}</span>
          <Button size="sm" color="secondary" variant="ghost" onClick={() => app.loadClips()}>
            Réessayer
          </Button>
        </div>
      ) : null}

      {a.clips.length === 0 ? (
        !a.clipsLoading && !a.clipsLoadError ? (
          <EmptyMessage className="h-[60vh]">
            <EmptyMessage.Title>Aucun clip pour le moment.</EmptyMessage.Title>
            <EmptyMessage.Description>
              Activer le replay, puis appuyer sur{' '}
              {hotkeyCaps.length > 0 ? (
                <span className="inline-flex items-center gap-1 align-middle">
                  {hotkeyCaps.map((cap, i) => (
                    <span key={i} className="inline-flex items-center gap-1">
                      {i > 0 && <span className="text-xs text-tertiary">+</span>}
                      <kbd className="rounded-sm border border-default bg-surface-secondary px-1.5 py-0.5 text-xs font-semibold text-default">
                        {cap}
                      </kbd>
                    </span>
                  ))}
                </span>
              ) : (
                'le raccourci du clip'
              )}{' '}
              en jouant.
            </EmptyMessage.Description>
          </EmptyMessage>
        ) : null
      ) : visible.length === 0 ? (
        <EmptyMessage className="h-[60vh]">
          <EmptyMessage.Title>
            {a.clipsLoading ? 'Recherche dans les clips chargés…' : 'Aucun clip ne correspond.'}
          </EmptyMessage.Title>
          <EmptyMessage.Description>
            Essayer un autre titre ou un autre jeu, ou effacer les filtres.
          </EmptyMessage.Description>
          <Button color="secondary" variant="outline" onClick={() => changeCriteria(() => app.resetLibraryFilters())}>
            Effacer les filtres
          </Button>
        </EmptyMessage>
      ) : (
        <div
          ref={setGridEl}
          className="grid grid-cols-[repeat(auto-fill,minmax(min(230px,100%),1fr))] gap-x-4 gap-y-6 [overflow-anchor:none]"
          style={{ paddingTop: windowed.top, paddingBottom: windowed.bottom }}>
          {mounted.map((clip, localIndex) => {
            const index = windowed.start + localIndex;
            return (
              <div key={clip.id} className="relative min-w-0 focus-within:z-[1] hover:z-[1]" data-index={index}>
                <ClipCard
                  clip={clip}
                  clipDir={a.clipDir}
                  selected={index === a.selected}
                  selectable={selecting}
                  checked={checked.has(clip.id)}
                  ontoggle={(range) => toggleCheck(clip.id, range)}
                  onselect={(event) => pick(clip, index, event)}
                  onfavorite={() => app.setFavorite(clip.id, !clip.favorite)}
                  onrename={(title) => app.rename(clip.id, title)}
                  oncopy={() => app.copyClip(clip.id)}
                  onreveal={() => app.reveal(clip.id)}
                  ondelete={() => {
                    app.selected = index;
                    askDelete([clip]);
                  }}
                />
                {previewing === clip.id && (
                  <video
                    className="pointer-events-none absolute top-0 left-0 aspect-[16/10] w-full rounded-xl bg-black object-cover"
                    src={clipUrl(a.clipDir, clip)}
                    autoPlay
                    loop
                    muted
                  />
                )}
              </div>
            );
          })}
        </div>
      )}

      <Dialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setDeleting(null);
        }}
        title={
          deleting === null ? '' : deleting.length === 1 ? 'Supprimer ce clip ?' : `Supprimer ${deleting.length} clips ?`
        }
        actions={
          <>
            <Button color="secondary" variant="ghost" size="sm" disabled={busy} onClick={() => setDeleting(null)}>
              Conserver
            </Button>
            <Button color="danger" size="sm" disabled={busy} onClick={confirmDelete}>
              <Trash />
              {busy
                ? 'Suppression…'
                : deleting?.length === 1
                  ? 'Supprimer'
                  : `Tout supprimer (${deleting?.length ?? 0})`}
            </Button>
          </>
        }>
        {deleting && (
          <>
            <p>
              {deleting.length === 1 ? (
                <>
                  La suppression de <strong className="text-default">{deleting[0].title}</strong> efface la vidéo, ses
                  métadonnées et sa miniature.
                </>
              ) : (
                <>
                  La suppression de ces <strong className="text-default">{deleting.length} clips</strong> efface les
                  vidéos, leurs métadonnées et leurs miniatures.
                </>
              )}{' '}
              Cette action est irréversible.
            </p>
            {a.deleteProgress && (
              <p className="mt-2" role="status">
                Suppression en cours&nbsp;: {a.deleteProgress.done} sur {a.deleteProgress.total}…
              </p>
            )}
          </>
        )}
      </Dialog>
    </>
  );
}
