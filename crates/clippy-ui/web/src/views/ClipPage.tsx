import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { useEffect, useRef, useState } from 'react';
import { Dialog } from '../components/ui/Dialog';
import { ChevronLeft, ChevronRight, Folder, Pencil, Scissors, Star, StarFilled, Trash } from '../components/icons';
import { Timeline } from '../components/Timeline';
import { VideoPlayer, type VideoPlayerHandle } from '../components/VideoPlayer';
import { app, useApp } from '../lib/app';
import { clipUrl, counterLabel, formatBytes, formatDuration } from '../lib/clips';
import { shouldHandleKey, sliderOwnsKey } from '../lib/keys';
import { trimRangeError } from '../lib/state';
import { formatClock } from '../lib/timeline';

const PRESS = 'transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]';

export function ClipPage() {
  const a = useApp();
  const player = useRef<VideoPlayerHandle>(null);
  const [renaming, setRenaming] = useState(false);
  const [draftTitle, setDraftTitle] = useState('');
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [keyframes, setKeyframes] = useState<number[]>([]);
  const [inMs, setInMs] = useState(0);
  const [outMs, setOutMs] = useState(0);
  const [playheadMs, setPlayheadMs] = useState(0);
  const [exporting, setExporting] = useState(false);

  const clip = a.current;

  // Read off `clip` as primitives rather than tracking the object: `rename` and
  // `setFavorite` both replace the ClipMeta in `app.clips` with a fresh object,
  // so an effect that depended on `clip` itself would throw away in/out points
  // and re-fetch the ticks every time someone starred the clip they were
  // halfway through trimming.
  const clipId = clip?.id ?? null;
  const clipDurationMs = clip?.duration_ms ?? 0;

  // `library::scan` adopts a bare .mp4 with `duration_ms: 0`, which `clamp_range`
  // refuses to trim. So the trim chrome is withheld and the seek band is not.
  const canTrim = clipDurationMs > 0;

  // Why the current range cannot be exported, or null when it can. The mistake
  // is named where it is made (the readout and the disabled button) rather than
  // by silently dragging the partner handle along. Same function as
  // `exportTrim`'s refusal, so the greyed button and Ctrl+E cannot disagree.
  const rangeError = canTrim ? trimRangeError(inMs, outMs) : null;

  // Re-fetch whenever the page shows a different clip. Prev/next is a UI-side
  // repoint of the same component, so without this the bar would keep the
  // previous clip's ticks and trim points. The cancel flag stops a stale fetch
  // landing on the wrong clip, and the list is cleared first: holding the old
  // one would draw the previous clip's ticks rescaled to this clip's duration.
  useEffect(() => {
    if (!clipId) return;
    setInMs(0);
    setOutMs(clipDurationMs);
    setPlayheadMs(0);
    setKeyframes([]);
    let stale = false;
    void app.keyframesFor(clipId).then((found) => {
      if (!stale) setKeyframes(found);
    });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId, clipDurationMs]);

  /**
   * The only way this page opens the delete dialog, and it leaves fullscreen
   * first: the fullscreen element is the player's stage, and the dialog is
   * mounted outside it, so the browser would paint the stage over it.
   */
  const askToDelete = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    setConfirmingDelete(true);
  };

  const exportTrim = async () => {
    // `exporting` is the double-fire guard: a double click or leaning on Ctrl+E
    // would otherwise put two identical trims in the library.
    if (!clip || exporting) return;
    // Ctrl+E has no button to grey out, so the zero-duration clip is refused
    // here in plain words rather than by silently doing nothing.
    if (!canTrim) {
      app.toast('error', 'Impossible de rogner ce clip : il n’a pas de durée.');
      return;
    }
    setExporting(true);
    try {
      await app.exportTrim(clip.id, inMs, outMs);
    } finally {
      setExporting(false);
    }
  };

  const startRename = () => {
    if (!clip) return;
    setDraftTitle(clip.title);
    setRenaming(true);
  };

  const commitRename = async () => {
    if (clip && draftTitle.trim()) await app.rename(clip.id, draftTitle.trim());
    setRenaming(false);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (renaming) {
      if (e.key === 'Escape') setRenaming(false);
      return;
    }
    // Every key is dropped while the dialog is up so the shortcuts below cannot
    // act on a different clip. Escape is the dialog's own business.
    if (confirmingDelete) return;
    // Ahead of `shouldHandleKey` so the one shortcut with no button to fall
    // back on cannot be taken away by whatever holds focus. `!altKey`: Windows
    // reports AltGr as Ctrl+Alt, and an unmapped AltGr combination must not
    // fire a full export.
    if (e.key === 'e' && e.ctrlKey && !e.altKey) {
      e.preventDefault();
      void exportTrim();
      return;
    }
    // A focused slider owns Left and Right (keyframes, a tenth of a second,
    // five seconds). Belt and braces: Timeline stops those events itself.
    if (sliderOwnsKey(e.target, e.key)) return;
    // Every button owns its own Space and Enter; arrows always fall through.
    if (!shouldHandleKey(e.target, e.key)) return;
    switch (e.key) {
      case 'Escape':
        app.view = 'grid';
        break;
      case 'ArrowLeft':
        e.preventDefault();
        app.step(-1);
        break;
      case 'ArrowRight':
        e.preventDefault();
        app.step(1);
        break;
      // Volume, as in most players. A focused slider stops its arrows before
      // they reach this handler.
      case 'ArrowUp':
        e.preventDefault();
        player.current?.stepVolume(1);
        break;
      case 'ArrowDown':
        e.preventDefault();
        player.current?.stepVolume(-1);
        break;
      case ' ':
        e.preventDefault();
        player.current?.toggle();
        break;
      // Set from the playhead: the point of `i`/`o` is to mark the frame you
      // are looking at.
      case 'i':
        setInMs(playheadMs);
        break;
      case 'o':
        setOutMs(playheadMs);
        break;
      case 'Delete':
        askToDelete();
        break;
    }
  };

  // The window listener is registered once and always calls the latest handler.
  const handler = useRef(onKeyDown);
  handler.current = onKeyDown;
  useEffect(() => {
    const fn = (e: KeyboardEvent) => handler.current(e);
    window.addEventListener('keydown', fn);
    return () => window.removeEventListener('keydown', fn);
  }, []);

  if (!clip) return null;

  return (
    <>
      <header className="mb-3 flex items-center gap-1.5">
        <Button color="secondary" variant="ghost" size="sm" className={PRESS} onClick={() => (app.view = 'grid')}>
          <ChevronLeft />
          Clips
        </Button>
        <span className="ml-auto" />
        <span className="mr-1 text-xs text-tertiary tabular-nums">{counterLabel(a.selected, a.visible.length)}</span>
        <Button
          color="secondary"
          variant="ghost"
          size="sm"
          uniform
          aria-label="Clip précédent"
          className={PRESS}
          disabled={a.selected === 0}
          onClick={() => app.step(-1)}>
          <ChevronLeft />
        </Button>
        <Button
          color="secondary"
          variant="ghost"
          size="sm"
          uniform
          aria-label="Clip suivant"
          className={PRESS}
          disabled={a.selected >= a.visible.length - 1}
          onClick={() => app.step(1)}>
          <ChevronRight />
        </Button>
      </header>

      <VideoPlayer
        ref={player}
        src={clipUrl(a.clipDir, clip.id)}
        onTime={setPlayheadMs}
        timeline={(playerDurationMs) => (
          // A clip with no duration still gets the band, and still cannot be
          // trimmed on it: the track and the playhead are there to seek with,
          // the handles are not. The player read a real duration off the file,
          // so seeking has something to scale against. Keyframes go with the
          // handles -- they exist only to snap the In point.
          <Timeline
            durationMs={canTrim ? clip.duration_ms : playerDurationMs}
            trimmable={canTrim}
            playheadMs={playheadMs}
            keyframes={canTrim ? keyframes : []}
            inMs={inMs}
            outMs={outMs}
            onChange={(i, o) => {
              setInMs(i);
              setOutMs(o);
            }}
            onSeek={(ms) => player.current?.seekTo(ms)}
          />
        )}
      />

      {canTrim && (
        <div className="mt-3 flex items-center gap-2.5">
          <span className={`text-xs tabular-nums ${rangeError !== null ? 'text-[var(--color-text-danger)]' : 'text-secondary'}`}>
            Point d’entrée <b className="font-semibold">{formatClock(inMs)}</b> &middot; Point de sortie{' '}
            <b className="font-semibold">{formatClock(outMs)}</b> &middot; Sélection{' '}
            <b className="font-semibold">{formatClock(outMs - inMs)}</b>
            {rangeError && <> &middot; {rangeError}</>}
          </span>
          <span className="ml-auto" />
          {/* `title` carries the reason a disabled button is disabled; the
              readout to its left is the other half of the answer. */}
          <Button
            color="primary"
            size="sm"
            className={PRESS}
            disabled={exporting || rangeError !== null}
            title={rangeError ?? ''}
            onClick={() => void exportTrim()}>
            <Scissors />
            {exporting ? 'Exportation…' : 'Exporter le clip rogné'}
          </Button>
        </div>
      )}

      <p className="mt-3.5 mb-1 text-xs text-tertiary tabular-nums">
        {clip.width}×{clip.height} &middot; {clip.fps}&nbsp;fps &middot; {formatDuration(clip.duration_ms)} &middot;{' '}
        {formatBytes(clip.bytes)} &middot; {clip.encoder}
        {clip.has_audio ? ' + audio' : ''}
      </p>
      <p className="mb-3 text-xs text-tertiary">
        I / O&nbsp;: points d’entrée et de sortie &middot; Ctrl+E&nbsp;: exporter &middot; ↑ ↓&nbsp;: volume &middot;
        Suppr&nbsp;: supprimer
      </p>

      <div className="flex flex-wrap items-center gap-1.5">
        {renaming ? (
          <>
            <div className="min-w-0 flex-1">
              <Input
                autoFocus
                size="sm"
                aria-label="Titre du clip"
                value={draftTitle}
                onChange={(e) => setDraftTitle(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && void commitRename()}
              />
            </div>
            <Button color="primary" size="sm" className={PRESS} onClick={() => void commitRename()}>
              Enregistrer
            </Button>
            <Button color="secondary" variant="ghost" size="sm" className={PRESS} onClick={() => setRenaming(false)}>
              Annuler
            </Button>
          </>
        ) : (
          <>
            {/* Worded buttons, not bare icons: an icon alone leaves a guess
                about what a folder or a pencil does. One star on this page, and
                it is the toggle. */}
            <span className="mr-auto min-w-0 truncate text-sm font-semibold">{clip.title}</span>
            <Button
              color="secondary"
              variant="outline"
              size="sm"
              className={PRESS}
              onClick={() => void app.setFavorite(clip.id, !clip.favorite)}>
              {clip.favorite ? <StarFilled className="text-amber-400" /> : <Star />}
              {clip.favorite ? 'Retirer des favoris' : 'Ajouter aux favoris'}
            </Button>
            <Button color="secondary" variant="outline" size="sm" className={PRESS} onClick={startRename}>
              <Pencil />
              Renommer
            </Button>
            <Button color="secondary" variant="outline" size="sm" className={PRESS} onClick={() => void app.reveal(clip.id)}>
              <Folder />
              Afficher dans le dossier
            </Button>
            <span className="mx-1 h-[18px] w-px bg-[var(--color-border-strong)]" aria-hidden="true" />
            <Button color="danger" variant="soft" size="sm" className={PRESS} onClick={askToDelete}>
              <Trash />
              Supprimer
            </Button>
          </>
        )}
      </div>

      <Dialog
        open={confirmingDelete}
        onOpenChange={setConfirmingDelete}
        title="Supprimer ce clip ?"
        actions={
          <>
            <Button color="secondary" variant="ghost" size="sm" className={PRESS} onClick={() => setConfirmingDelete(false)}>
              Conserver
            </Button>
            <Button
              color="danger"
              size="sm"
              className={PRESS}
              onClick={async () => {
                setConfirmingDelete(false);
                await app.remove(clip.id);
              }}>
              <Trash />
              Supprimer
            </Button>
          </>
        }>
        <p>
          La suppression de <strong className="text-default">{clip.title}</strong> efface le fichier mp4, ses métadonnées et sa
          miniature. Cette action est irréversible.
        </p>
      </Dialog>
    </>
  );
}
