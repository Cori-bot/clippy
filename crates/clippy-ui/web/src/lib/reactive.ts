import { useSyncExternalStore } from 'react';

/**
 * A store that React components can subscribe to without the store knowing
 * about React.
 *
 * `reactive(store)` returns a proxy of the same object. Every assignment to one
 * of its fields bumps a version and tells the subscribers, batched into one
 * notification per microtask so a method that sets six fields renders once.
 * Methods and getters run with `this` bound to the proxy, which is what makes
 * `this.x = ...` inside them observable.
 *
 * Only assignment is observed. The stores replace arrays, sets and objects
 * instead of mutating them in place, which is also what keeps a component's
 * snapshot honest.
 */
export interface Observable {
  subscribe(listener: () => void): () => void;
  version(): number;
}

const handles = new WeakMap<object, Observable>();

export function reactive<T extends object>(target: T): T {
  const listeners = new Set<() => void>();
  let version = 0;
  let queued = false;
  const notify = () => {
    version++;
    if (queued) return;
    queued = true;
    queueMicrotask(() => {
      queued = false;
      for (const l of [...listeners]) l();
    });
  };
  const proxy = new Proxy(target, {
    set(obj, key, value, receiver) {
      const changed = !Object.is(Reflect.get(obj, key, receiver), value);
      const ok = Reflect.set(obj, key, value, receiver);
      if (changed) notify();
      return ok;
    },
  });
  handles.set(proxy, {
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    version: () => version,
  });
  return proxy;
}

/** Re-renders the calling component whenever the store changes, and hands the store back. */
export function useStore<T extends object>(store: T): T {
  const handle = handles.get(store);
  if (!handle) throw new Error('useStore needs a store made by reactive()');
  useSyncExternalStore(handle.subscribe, handle.version);
  return store;
}
