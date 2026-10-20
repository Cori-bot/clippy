import { useEffect, useRef, useState } from 'react';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { ShotCard } from '../components/ShotCard';
import { ShotViewer } from '../components/ShotViewer';
import { Dialog } from '../components/ui/Dialog';
import { Trash } from '../components/icons';
import { app, useApp } from '../lib/app';
import { shouldHandleKey, moveSelection, CARD_CONTROL_SELECTOR } from '../lib/keys';
import { formatCombo } from '../lib/ui';
import { formatShotDate } from '../lib/shots';
import type { ShotMeta } from '../lib/types';

export function Shots() {
  const a = useApp();
  const [viewing, setViewing] = useState(false);
  // The screenshot waiting on the delete dialog: the tile button, the context
  // menu and the Delete key all land here, so none removes a file unasked.
  const [deleting, setDeleting] = useState<ShotMeta | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const columns = useRef(4);

  const hotkeyCaps = formatCombo(a.shotHotkey);

  useEffect(() => {
    void app.loadShots();
  }, []);

  /**
   * Scrolls the selected tile fully into view and no further. Called where a
   * key moves the selection, not on every index change: the index also shifts
   * when a new screenshot is saved, which must not yank the page back.
   */
  function revealSelected() {
    const tile = gridRef.current?.children[app.shotSelected];
    if (tile instanceof HTMLElement) tile.scrollIntoView({ block: 'nearest' });
  }

  // Once on arrival too: the selection outlives a trip to another tab.
  const hasGrid = a.shots.length > 0;
  useEffect(() => {
    if (hasGrid) revealSelected();
  }, [hasGrid]);

  useEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      columns.current = Math.max(1, getComputedStyle(el).gridTemplateColumns.split(' ').length);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasGrid]);

  const stateRef = useRef({ viewing, deleting });
  stateRef.current = { viewing, deleting };

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // The viewer owns every key while open, and the delete dialog while it asks.
      if (stateRef.current.viewing || stateRef.current.deleting) return;
      const grid = gridRef.current;
      const target = e.target instanceof Element ? e.target : null;
      const ownedByCardControl = !!target?.closest(CARD_CONTROL_SELECTOR);
      const insideGrid = !ownedByCardControl && !!grid && e.target instanceof Node && grid.contains(e.target);
      if (!shouldHandleKey(e.target, e.key, insideGrid)) return;

      if (e.key === 'Enter') {
        if (app.shots.length === 0) return;
        e.preventDefault();
        setViewing(true);
        return;
      }
      if (e.key === 'Delete') {
        const shot = app.shots[app.shotSelected];
        if (!shot) return;
        e.preventDefault();
        setDeleting(shot);
        return;
      }
      const next = moveSelection(app.shotSelected, e.key, app.shots.length, columns.current);
      if (next !== app.shotSelected) {
        e.preventDefault();
        app.shotSelected = next;
        revealSelected();
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  function confirmDelete() {
    const shot = deleting;
    setDeleting(null);
    if (shot) void app.deleteShot(shot.id);
  }

  return (
    <div className="relative min-h-full">
      {a.shots.length === 0 ? (
        <div className="flex flex-col items-center gap-1.5 py-16 text-secondary">
          <p>Aucune capture d’écran pour le moment.</p>
          <p className="text-sm text-tertiary">
            Appuyer sur{' '}
            {hotkeyCaps.length > 0 ? (
              <span className="inline-flex items-center gap-1 align-middle">
                {hotkeyCaps.map((cap, i) => (
                  <span key={i} className="inline-flex items-center gap-1">
                    {i > 0 && <span className="text-xs text-tertiary">+</span>}
                    <kbd className="rounded-md border border-default bg-surface-secondary px-1.5 py-0.5 font-sans text-xs font-semibold text-default">
                      {cap}
                    </kbd>
                  </span>
                ))}
              </span>
            ) : (
              'le raccourci de capture d’écran'
            )}{' '}
            tant que le replay est actif.
          </p>
        </div>
      ) : (
        <div ref={gridRef} className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3.5 [&>*]:scroll-my-3.5">
          {a.shots.map((shot, i) => (
            <ShotCard
              key={shot.id}
              shot={shot}
              selected={i === a.shotSelected}
              onSelect={() => (app.shotSelected = i)}
              onDelete={() => setDeleting(shot)}
              onOpen={() => {
                app.shotSelected = i;
                setViewing(true);
              }}
            />
          ))}
        </div>
      )}

      {viewing && <ShotViewer onClose={() => setViewing(false)} />}

      <Dialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title="Supprimer cette capture d’écran ?"
        actions={
          <>
            <Button color="secondary" variant="ghost" size="sm" onClick={() => setDeleting(null)}>
              Conserver
            </Button>
            <Button color="danger" size="sm" onClick={confirmDelete}>
              <Trash />
              Supprimer
            </Button>
          </>
        }>
        <p>
          La suppression de la capture d’écran du <strong>{deleting ? formatShotDate(deleting.created) : ''}</strong>{' '}
          efface l’image et sa miniature. Cette action est irréversible.
        </p>
      </Dialog>
    </div>
  );
}
