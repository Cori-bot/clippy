import { app, exportJobs, updates } from './state';
import { useStore } from './reactive';

export { app, exportJobs, updates };

/** The app store, re-rendering the calling component whenever it changes. */
export function useApp() {
  return useStore(app);
}

/** The export survives navigating away from the editor. */
export function useExportJobs() {
  return useStore(exportJobs);
}

/** The updater store, re-rendering the calling component whenever it changes. */
export function useUpdates() {
  return useStore(updates);
}
