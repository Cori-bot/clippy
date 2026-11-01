import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Input } from '@openai/apps-sdk-ui/components/Input';
import { Select } from '@openai/apps-sdk-ui/components/Select';
import { Slider } from '@openai/apps-sdk-ui/components/Slider';
import { Menu } from '@openai/apps-sdk-ui/components/Menu';
import { Undo, ArrowRotateCw } from '@openai/apps-sdk-ui/components/Icon';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Dialog } from '../components/ui/Dialog';
import { Check, ChevronLeft, ChevronRight, Copy, Dots, Folder, Pencil, Scissors, Star, StarFilled, Trash } from '../components/icons';
import { Logo } from '../components/Logo';
import { ExportJobPanel } from '../components/ExportJobPanel';
import { ReplayControl } from '../components/ReplayControl';
import { Timeline } from '../components/Timeline';
import { TrackRows } from '../components/TrackRows';
import { VideoPlayer, type VideoPlayerHandle } from '../components/VideoPlayer';
import { app, exportJobs, useApp, useExportJobs } from '../lib/app';
import { clipUrl, counterLabel, trackUrl, formatBytes, formatDuration } from '../lib/clips';
import { call } from '../lib/ipc';
import { isTypingTarget, shouldHandleKey, sliderOwnsKey } from '../lib/keys';
import { useEditorDraft } from '../lib/editor-draft';
import { trimRangeError } from '../lib/state';
import { formatClock, formatPreciseClock, nearestFrameBoundary, snapStart } from '../lib/timeline';
import { buildExportTracks, editableTracks } from '../lib/tracks';
import { capError, SIZE_PRESETS } from '../lib/share';
import type { ExportMode, ExportOutput, FrameIndex } from '../lib/types';

const PRESS = 'transition-transform duration-150 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]';

export function ClipPage() {
  const a = useApp();
  const jobs = useExportJobs();
  const player = useRef<VideoPlayerHandle>(null);
  const [renaming, setRenaming] = useState(false);
  const [draftTitle, setDraftTitle] = useState('');
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [keyframeResult, setKeyframeResult] = useState<{identity: string; points: number[]}>();
  const [playheadMs, setPlayheadMs] = useState(0);
  const [exporting, setExporting] = useState(false);
  const [output, setOutput] = useState<ExportOutput>('share');
  const [chosenMode, setMode] = useState<ExportMode>('precise');
  /** A Discord-style size cap in MB, or null for the source quality. */
  const [sizeLimit, setSizeLimit] = useState<number | null>(null);
  // A size cap reencodes into one mix, so it implies a precise share.
  /** Centre of the 9:16 crop in thousandths of the width; null = source framing. */
  const [verticalCenter, setVerticalCenter] = useState<number | null>(null);
  const mode: ExportMode = sizeLimit !== null || verticalCenter !== null ? 'precise' : chosenMode;
  const effectiveOutput: ExportOutput = sizeLimit !== null ? 'share' : output;
  const [frameResult, setFrameResult] = useState<{ identity: string; index: FrameIndex }>();
  const [framesLoading, setFramesLoading] = useState(false);
  const [framesError, setFramesError] = useState<string | null>(null);
  const [framesAttempt, setFramesAttempt] = useState(0);
  const exportLock = useRef(false);
  // Per-track audio: levels chosen by the user, and the per-track files the
  // daemon prepared for playback (`library.tracks`).
  const [trackResult, setTrackResult] = useState<{identity: string; files: Record<number, string>}>();
  const [tracksLoading, setTracksLoading] = useState(false);
  const [tracksError, setTracksError] = useState(false);

  const clip = a.current;
  const tracks = useMemo(() => editableTracks(clip?.audio_tracks), [clip?.audio_tracks]);
  const tracksKey = tracks.map((t) => t.index).join(',');

  // Read off `clip` as primitives rather than tracking the object: `rename` and
  // `setFavorite` both replace the ClipMeta in `app.clips` with a fresh object,
  // so an effect that depended on `clip` itself would throw away in/out points
  // and re-fetch the ticks every time someone starred the clip they were
  // halfway through trimming.
  const clipId = clip?.id ?? null;
  const clipDurationMs = clip?.duration_ms ?? 0;
  const frameIdentity = JSON.stringify([a.clipDir, clipId, clipDurationMs]);
  const frameIndex = frameResult?.identity === frameIdentity ? frameResult.index : undefined;
  const trackFiles = trackResult?.identity === frameIdentity ? trackResult.files : {};
  const keyframes = keyframeResult?.identity === frameIdentity ? keyframeResult.points : [];
  const clock = mode === 'precise' ? formatPreciseClock : formatClock;
  const editor = useEditorDraft({ directory: a.clipDir, id: clipId ?? '', durationMs: clipDurationMs, audio: clip?.audio_tracks ?? [] });
  const { inMs: draftIn, outMs: draftOut, tracks: trackStates } = editor.recipe;
  const inMs = mode === 'fast' ? snapStart(draftIn, keyframes) : frameIndex ? nearestFrameBoundary(draftIn, frameIndex) : draftIn;
  const outMs = mode === 'precise' && frameIndex ? nearestFrameBoundary(draftOut, frameIndex, true) : draftOut;
  const setRange = (nextIn: number, nextOut: number, group: string) => editor.update({ ...editor.recipe, inMs: nextIn, outMs: nextOut }, group);

  // `library::scan` adopts a bare .mp4 with `duration_ms: 0`, which `clamp_range`
  // refuses to trim. So the trim chrome is withheld and the seek band is not.
  const canTrim = clipDurationMs > 0;

  // Why the current range cannot be exported, or null when it can. The mistake
  // is named where it is made (the readout and the disabled button) rather than
  // by silently dragging the partner handle along. Same function as
  // `exportTrim`'s refusal, so the greyed button and Ctrl+E cannot disagree.
  const rangeError = canTrim ? trimRangeError(inMs, outMs, mode) : null;
  const sizeError = canTrim ? capError(sizeLimit, outMs - inMs, clip?.has_audio ?? false) : null;

  // Re-fetch whenever the page shows a different clip. Prev/next is a UI-side
  // repoint of the same component, so without this the bar would keep the
  // previous clip's ticks and trim points. The cancel flag stops a stale fetch
  // landing on the wrong clip, and the list is cleared first: holding the old
  // one would draw the previous clip's ticks rescaled to this clip's duration.
  useEffect(() => {
    if (!clipId) return;
    setPlayheadMs(0);
    setKeyframeResult(undefined);
    let stale = false;
    void app.keyframesFor(clipId).then((found) => {
      if (!stale) setKeyframeResult({identity: frameIdentity, points: found});
    });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId, frameIdentity]);

  useEffect(() => {
    setFrameResult(undefined);
    setFramesError(null);
    if (!clipId) return;
    let stale = false;
    setFramesLoading(true);
    void call<FrameIndex>('library.frames', { clip_id: clipId }).then(index => {
      if (stale) return;
      const points = index.timestamps_100ns;
      if (!Array.isArray(points) || points.length === 0 || !Number.isSafeInteger(index.end_100ns)
        || points.some((point, i) => !Number.isSafeInteger(point) || point < 0 || (i > 0 && point <= points[i - 1]))
        || index.end_100ns <= points[points.length - 1]) throw new Error('Index des images invalide.');
      setFrameResult({identity: frameIdentity, index});
    }).catch(error => { if (!stale) setFramesError(String(error)); })
      .finally(() => { if (!stale) setFramesLoading(false); });
    return () => { stale = true; };
  }, [clipId, frameIdentity, framesAttempt]);

  // Fresh levels and a fresh file list for every clip. Fetched once per clip;
  // the daemon caches the remux, so only the first visit waits.
  useEffect(() => {
    setTrackResult(undefined);
    setTracksError(false);
    if (!clipId || tracks.length === 0) {
      setTracksLoading(false);
      return;
    }
    let stale = false;
    setTracksLoading(true);
    call<{ tracks: { index: number; name: string; file: string }[] }>('library.tracks', { clip_id: clipId })
      .then((res) => {
        if (stale) return;
        const files: Record<number, string> = {};
        for (const t of res.tracks) files[t.index] = t.file;
        setTrackResult({identity: frameIdentity, files});
        setTracksError(!tracks.every((t) => files[t.index]));
      })
      .catch(() => {
        if (!stale) setTracksError(true);
      })
      .finally(() => {
        if (!stale) setTracksLoading(false);
      });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId, tracksKey, frameIdentity]);

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
    // A ref locks immediately, including two commands before React renders.
    if (!clip || exportLock.current || jobs.active) return;
    // Ctrl+E has no button to grey out, so the zero-duration clip is refused
    // here in plain words rather than by silently doing nothing.
    if (!canTrim) {
      app.toast('error', 'Impossible de rogner ce clip : il n’a pas de durée.');
      return;
    }
    if (rangeError || sizeError) {
      app.toast('error', (rangeError ?? sizeError)!);
      return;
    }
    if (mode === 'precise' && !frameIndex) {
      app.toast('error', framesError ?? 'L’index des images est encore en préparation.');
      return;
    }
    exportLock.current = true;
    setExporting(true);
    try {
      const audio = buildExportTracks(tracks, trackStates);
      await exportJobs.start({ clip_id: clip.id, start_ms: Math.round(inMs), end_ms: Math.round(outMs), mode, output: effectiveOutput,
        ...(audio ? { tracks: audio } : {}), ...(sizeLimit !== null ? { size_limit_mb: sizeLimit } : {}),
        ...(verticalCenter !== null ? { vertical_center: verticalCenter } : {}) });
    } finally {
      exportLock.current = false;
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
    if (e.defaultPrevented || e.isComposing || isTypingTarget(e.target)
      || (e.target instanceof Element && e.target.closest('[role="dialog"],dialog,[role="menu"]'))) return;
    if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === 'z') {
      e.preventDefault();
      if (e.shiftKey) editor.redo(); else editor.undo();
      return;
    }
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
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    switch (e.key) {
      case ',':
      case '.':
        if (e.target instanceof Element && e.target.closest('[role="slider"],input[type="range"]')) return;
        e.preventDefault();
        player.current?.stepFrame(e.key === ',' ? -1 : 1);
        break;
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
        setRange(player.current?.currentFrame() ?? playheadMs, outMs, 'in');
        break;
      case 'o':
        setRange(inMs, player.current?.currentFrame() ?? playheadMs, 'out');
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

  // Separate playback only once every track file is known; until then (and on
  // failure) the video plays its own mixed audio.
  const separateTracks =
    tracks.length > 0 && !tracksLoading && !tracksError && tracks.every((t) => trackFiles[t.index]);

  if (!clip) return null;

  return (
    <>
      <div className="editor-workspace">
        <header className="editor-toolbar">
          <div className="editor-brand"><Logo height={25} /><span>Clippy</span></div>
          <Button color="secondary" variant="ghost" size="sm" onClick={() => (app.view = 'grid')}>
            <ChevronLeft /><span className="editor-back-label">Bibliothèque</span>
          </Button>
          <div className="editor-title"><h1 title={clip.title}>{clip.title}</h1></div>
          <div className="editor-navigation">
            <Button color="secondary" variant="ghost" size="sm" uniform aria-label="Clip précédent" disabled={a.selected === 0} onClick={() => app.step(-1)}><ChevronLeft /></Button>
            <span>{counterLabel(a.selected, a.visible.length)}</span>
            <Button color="secondary" variant="ghost" size="sm" uniform aria-label="Clip suivant" disabled={a.selected >= a.visible.length - 1} onClick={() => app.step(1)}><ChevronRight /></Button>
          </div>
          <div className="editor-history" aria-label="Historique du montage">
            <Button color="secondary" variant="ghost" size="sm" uniform aria-label="Annuler le montage" disabled={!editor.canUndo} title="Annuler (Ctrl+Z)" onClick={editor.undo}><Undo /></Button>
            <Button color="secondary" variant="ghost" size="sm" uniform aria-label="Rétablir le montage" disabled={!editor.canRedo} title="Rétablir (Ctrl+Maj+Z)" onClick={editor.redo}><ArrowRotateCw /></Button>
          </div>
          <span className={`editor-save-state ${editor.status === 'unavailable' ? 'text-danger' : ''}`} role="status" title={editor.status === 'unavailable' ? 'Sauvegarde locale indisponible. Les réglages seront perdus en quittant ce clip.' : 'La vidéo originale est conservée.'}>
            {editor.status === 'saved' && <Check />}
            {editor.status === 'saved' ? 'Enregistré' : editor.status === 'pending' ? 'Enregistrement…' : editor.status === 'unavailable' ? 'Non enregistré' : editor.status === 'discarded' ? 'Montage réinitialisé' : 'Original conservé'}
          </span>
          <div onKeyDown={event => event.stopPropagation()}>
            <Menu>
              <Menu.Trigger><Button color="secondary" variant="ghost" size="sm" uniform aria-label="Actions du clip"><Dots /></Button></Menu.Trigger>
              <Menu.Content align="end" minWidth={220}>
                <Menu.Item onSelect={() => void app.setFavorite(clip.id, !clip.favorite)}><span className="flex items-center gap-2">{clip.favorite ? <StarFilled /> : <Star />}{clip.favorite ? 'Retirer des favoris' : 'Ajouter aux favoris'}</span></Menu.Item>
                <Menu.Item onSelect={startRename}><span className="flex items-center gap-2"><Pencil />Renommer</span></Menu.Item>
                <Menu.Item onSelect={() => void app.copyClip(clip.id)}><span className="flex items-center gap-2"><Copy />Copier le fichier</span></Menu.Item>
                <Menu.Item onSelect={() => void app.reveal(clip.id)}><span className="flex items-center gap-2"><Folder />Afficher dans le dossier</span></Menu.Item>
                <Menu.Item onSelect={editor.reset} disabled={!editor.changed}>Réinitialiser le montage</Menu.Item>
                <Menu.Separator />
                <Menu.Item onSelect={askToDelete}><span className="flex items-center gap-2 text-danger"><Trash />Supprimer</span></Menu.Item>
              </Menu.Content>
            </Menu>
          </div>
          <Button color="primary" size="sm" className="editor-export motion-control" disabled={!canTrim || exporting || jobs.active || rangeError !== null || sizeError !== null || (mode === 'precise' && !frameIndex)} title={rangeError ?? sizeError ?? (!canTrim ? 'La durée de ce clip est inconnue.' : '')} onClick={() => void exportTrim()}>
            <span className="motion-icon motion-nudge"><Scissors /></span>{exporting ? 'Préparation…' : jobs.active ? 'Export en cours' : 'Exporter'}
          </Button>
        </header>

        <VideoPlayer
          ref={player}
          frameIndex={frameIndex}
          src={clipUrl(a.clipDir, clip)}
          trackUrls={separateTracks ? tracks.map((t) => trackUrl(a.clipDir, clip, trackFiles[t.index])) : []}
          trackStates={trackStates}
          onTime={setPlayheadMs}
          verticalCenter={verticalCenter}
          onVerticalCenter={exporting || jobs.active ? undefined : setVerticalCenter}
          inspector={(
            <div className="editor-inspector">
              <h2>Montage</h2>
              <section className="editor-inspector-section">
                <h3>Sélection</h3>
                {canTrim ? <>
                  <dl className="editor-selection">
                    <div><dt>Entrée</dt><dd><button type="button" title="Aller au point d’entrée" onClick={() => mode === 'precise' && frameIndex ? player.current?.seekFrame(inMs) : player.current?.seekTo(inMs)}>{clock(inMs)}</button><Button color="secondary" variant="ghost" size="xs" aria-label="Définir l’entrée à l’image courante" title="Marquer l’entrée (I)" onClick={() => setRange(player.current?.currentFrame() ?? playheadMs, outMs, 'in')}>I</Button></dd></div>
                    <div><dt>Sortie</dt><dd><button type="button" title="Aller au point de sortie" onClick={() => player.current?.seekTo(outMs)}>{clock(outMs)}</button><Button color="secondary" variant="ghost" size="xs" aria-label="Définir la sortie à l’image courante" title="Marquer la sortie (O)" onClick={() => setRange(inMs, player.current?.currentFrame() ?? playheadMs, 'out')}>O</Button></dd></div>
                    <div><dt>Durée</dt><dd className="editor-duration">{clock(Math.max(0, outMs - inMs))}</dd></div>
                  </dl>
                  {rangeError && <p className="editor-notice text-danger" role="alert">{rangeError}</p>}
                  {(framesLoading || framesError) && <div className="editor-notice" role="status">{framesLoading ? 'Préparation des images…' : 'Index des images indisponible.'}{framesError && <Button size="xs" color="secondary" variant="ghost" onClick={() => setFramesAttempt(value => value + 1)}>Réessayer</Button>}</div>}
                </> : <p className="editor-notice">Durée inconnue. La lecture reste disponible.</p>}
              </section>
              <section className="editor-inspector-section">
                <h3>Audio</h3>
                {tracks.length ? <TrackRows tracks={tracks} states={trackStates} loading={tracksLoading} error={tracksError} onChange={(i, next) => editor.update({ ...editor.recipe, tracks: trackStates.map((s, k) => k === i ? next : s) }, `track:${i}`)} /> : <p className="editor-notice">{clip.has_audio ? 'Son intégré à la vidéo.' : 'Ce clip ne contient pas de son.'}</p>}
              </section>
              <section className="editor-inspector-section" onKeyDown={event => event.stopPropagation()}>
                <h3>Sortie</h3>
                <div className="editor-output-controls">
                  <Select aria-label="Taille du fichier" size="sm" value={sizeLimit === null ? 'source' : String(sizeLimit)} disabled={exporting || jobs.active} options={[
                    { value: 'source', label: 'Qualité d’origine', description: 'Aucune limite de taille.' },
                    ...SIZE_PRESETS.map(p => ({ value: String(p.mb), label: p.label, description: p.description })),
                  ]} onChange={option => setSizeLimit(option.value === 'source' ? null : Number(option.value))} />
                  <Select aria-label="Format de l’image" size="sm" value={verticalCenter === null ? 'source' : 'vertical'} disabled={exporting || jobs.active} options={[
                    { value: 'source', label: 'Format d’origine', description: 'Image entière.' },
                    { value: 'vertical', label: 'Vertical 9:16', description: 'TikTok, Shorts, Reels. Recadré dans l’image.' },
                  ]} onChange={option => setVerticalCenter(option.value === 'vertical' ? 500 : null)} />
                  {verticalCenter !== null && <div className="editor-crop-slider" role="group" aria-label="Cadrage horizontal" title="Position de la zone gardée, de la gauche (0 %) à la droite (100 %).">
                    <Slider label="Cadrage" value={Math.round(verticalCenter / 10)} min={0} max={100} step={1} unit="%" resetValue={50}
                      disabled={exporting || jobs.active} onChange={v => setVerticalCenter(v * 10)} />
                  </div>}
                  <Select aria-label="Précision de la coupe" size="sm" value={mode} disabled={exporting || jobs.active || sizeLimit !== null || verticalCenter !== null} options={[
                    { value: 'precise', label: 'Coupe précise', description: 'Respecte les images choisies. Réencode la vidéo.' },
                    { value: 'fast', label: 'Coupe rapide', description: 'Sans réencodage vidéo. Début aligné sur une image clé.' },
                  ]} onChange={option => setMode(option.value as ExportMode)} />
                  <Select aria-label="Type de fichier exporté" size="sm" disabled={exporting || jobs.active || sizeLimit !== null} value={effectiveOutput} options={[
                    { value: 'share', label: 'MP4 de partage', description: 'Une piste audio mixée. Original conservé.' },
                    { value: 'master', label: 'MP4 avec pistes audio', description: 'Pistes séparées dans la même vidéo.' },
                  ]} onChange={option => setOutput(option.value as ExportOutput)} />
                </div>
                {sizeError ? <p className="editor-notice text-danger" role="status">{sizeError}</p>
                  : sizeLimit !== null ? <p className="editor-notice">Coupe précise et MP4 de partage imposés. Débit et définition ajustés pour rester sous {sizeLimit} Mo.</p>
                  : verticalCenter !== null && <p className="editor-notice">Coupe précise imposée. Glissez le cadre sur la vidéo pour choisir la zone gardée.</p>}
              </section>
              <ExportJobPanel />
              <details className="editor-details"><summary>Fichier source</summary><p>{clip.width}×{clip.height} · {clip.fps} fps<br />{formatDuration(clip.duration_ms)} · {formatBytes(clip.bytes)}<br />{clip.encoder}{clip.has_audio ? ' + audio' : ''}</p></details>
              <details className="editor-details"><summary>Replay · {a.armed ? 'actif' : a.waiting ? 'en attente' : 'désactivé'}</summary><ReplayControl /></details>
            </div>
          )}
          timeline={(playerDurationMs) => (
            <Timeline
              src={clipUrl(a.clipDir, clip)}
              trackUrls={separateTracks ? tracks.map((t) => trackUrl(a.clipDir, clip, trackFiles[t.index])) : []}
              trackNames={tracks.map(t => t.name)}
              durationMs={canTrim ? mode === 'precise' && frameIndex ? frameIndex.end_100ns / 10000 : clip.duration_ms : playerDurationMs}
              trimmable={canTrim}
              playheadMs={playheadMs}
              keyframes={canTrim ? keyframes : []}
              frameIndex={mode === 'precise' ? frameIndex : undefined}
              inMs={inMs}
              outMs={outMs}
              onChange={(i, o) => setRange(i, o, 'trim')}
              onSeek={(ms) => player.current?.seekTo(ms)}
            />
          )}
        />
        <footer className="editor-statusbar"><span><kbd>,</kbd> / <kbd>.</kbd> Image par image<span className="editor-shortcut-extra"> · <kbd>I</kbd> / <kbd>O</kbd> Entrée / sortie</span></span><span>{clip.width} × {clip.height} · {clip.fps} fps</span><span><kbd>Ctrl</kbd> + <kbd>E</kbd> Exporter</span></footer>
      </div>

      <Dialog open={renaming} onOpenChange={setRenaming} title="Renommer le clip" actions={<><Button color="secondary" variant="ghost" size="sm" onClick={() => setRenaming(false)}>Annuler</Button><Button color="primary" size="sm" disabled={!draftTitle.trim()} onClick={() => void commitRename()}>Enregistrer</Button></>}>
        <Input autoFocus size="sm" aria-label="Titre du clip" value={draftTitle} onChange={(e) => setDraftTitle(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void commitRename()} />
      </Dialog>

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
