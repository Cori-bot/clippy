import { Button } from '@openai/apps-sdk-ui/components/Button';
import { useEffect } from 'react';
import { TitleBar } from './components/TitleBar';
import { Toasts } from './components/Toasts';
import { updates, useApp, useUpdates } from './lib/app';
import { isTypingTarget } from './lib/keys';
import { wireDaemon, wireUpdates } from './lib/state';
import { ClipPage } from './views/ClipPage';
import { DaemonDown } from './views/DaemonDown';
import { FirstRun } from './views/FirstRun';
import { Grid } from './views/Grid';
import { Rail } from './views/Rail';
import { Settings } from './views/Settings';
import { Games } from './views/Games';
import { Shots } from './views/Shots';

// The same page the Rust side checks every opened URL against.
const RELEASES_URL = 'https://github.com/OWNER/clippy/releases';

function UpdateBanner() {
  const u = useUpdates();
  const banner = u.banner;
  if (!banner) return null;

  // `href` stays so the target is visible on hover and can be copied, but the
  // click is handled in Rust: a Tauri webview opens no new window, so the
  // default action here would be nothing at all.
  const open = (url: string) => (e: React.MouseEvent) => {
    e.preventDefault();
    void updates.openReleasesPage(url);
  };

  return (
    <div
      className={`flex shrink-0 items-center gap-3 border-b border-subtle px-4 py-2 text-sm ${
        banner.error ? 'bg-surface-secondary text-secondary' : 'bg-info-soft-alpha'
      }`}>
      {banner.error ? (
        <>
          <span>{banner.error}</span>
          <a className="text-info underline" href={RELEASES_URL} onClick={open(RELEASES_URL)}>
            Télécharger manuellement
          </a>
        </>
      ) : u.busy ? (
        <span>{u.phase === 'downloading' ? `Téléchargement… ${u.percent} %` : 'Installation…'}</span>
      ) : (
        <>
          <span>Clippy {banner.version} est disponible</span>
          <a className="text-info underline" href={u.release?.notes_url} onClick={open(u.release?.notes_url ?? RELEASES_URL)}>
            Nouveautés
          </a>
          <span className="ml-auto" />
          <Button color="primary" size="xs" onClick={() => void updates.install()}>
            Mettre à jour
          </Button>
        </>
      )}
    </div>
  );
}

export function App() {
  const a = useApp();
  const u = useUpdates();

  useEffect(() => {
    wireDaemon();
    wireUpdates();
    // No webview right-click menu anywhere in the app: its "Save image as" and
    // "More tools" belong to a web page, not to Clippy. Cards open their own menu
    // and call preventDefault themselves; this catches everything else. A text
    // box keeps the native menu -- Cut, Copy and Paste are what anyone
    // right-clicking inside one is after.
    const onContextMenu = (e: MouseEvent) => {
      if (!isTypingTarget(e.target)) e.preventDefault();
    };
    window.addEventListener('contextmenu', onContextMenu);
    return () => window.removeEventListener('contextmenu', onContextMenu);
  }, []);

  return (
    <>
      <div className="flex h-screen flex-col overflow-hidden">
        <TitleBar />
        <UpdateBanner />

        {u.swapping ? (
          // The swap itself is why the daemon looks gone -- DaemonDown's Start
          // button would just be refused (an install already owns the pipe), so
          // for this window the banner above is the entire window.
          <div className="flex-1" />
        ) : !a.connected ? (
          <DaemonDown />
        ) : a.view === 'firstrun' ? (
          <FirstRun />
        ) : (
          <div className="flex min-h-0 flex-1">
            <Rail />
            <main className="relative flex-1 overflow-auto px-5 py-[18px]">
              {a.view === 'grid' && <Grid />}
              {a.view === 'clip' && <ClipPage />}
              {a.view === 'shots' && <Shots />}
              {a.view === 'games' && <Games />}
              {a.view === 'settings' && <Settings />}
            </main>
          </div>
        )}
      </div>
      <Toasts />
    </>
  );
}

