import { app, updates } from './state';
import { useStore } from './reactive';

export { app, updates };

/** The app store, re-rendering the calling component whenever it changes. */
export function useApp() {
  return useStore(app);
}

/** The updater store, re-rendering the calling component whenever it changes. */
export function useUpdates() {
  return useStore(updates);
}
