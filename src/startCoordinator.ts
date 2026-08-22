export type StartTask<T> = (isCurrent: () => boolean) => Promise<T>;

/**
 * Serialize session starts, deduplicate identical intents, and discard queued
 * intents superseded by a newer one. The task receives a freshness predicate
 * so an already-running backend call can avoid committing stale UI state.
 */
export function createLatestStartCoordinator<T>(staleValue: T) {
  let tail: Promise<unknown> = Promise.resolve();
  let latestSeq = 0;
  const pending = new Map<string, Promise<T>>();

  return {
    schedule(key: string, task: StartTask<T>): Promise<T> {
      const duplicate = pending.get(key);
      if (duplicate) return duplicate;

      const seq = ++latestSeq;
      const run = tail
        .catch(() => staleValue)
        .then(() => {
          if (seq !== latestSeq) return staleValue;
          return task(() => seq === latestSeq);
        });
      let tracked: Promise<T>;
      tracked = run.finally(() => {
        if (pending.get(key) === tracked) pending.delete(key);
      });
      pending.set(key, tracked);
      tail = tracked;
      return tracked;
    },
  };
}
