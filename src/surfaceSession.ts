import type { SurfaceKind } from "./surface";

/** Per-surface UI snapshot. The engine binds surface_kind immutably, so a
 *  Chat/Code/Work switch must start or resume a different session — but the
 *  transcript should stay on screen instead of flashing empty. */
export type SurfaceSessionSnapshot = {
  sessionId: string;
  items: unknown[];
  workspace: string;
  workWorkspaceId: string;
};

export type SurfaceSessionCache = Partial<Record<SurfaceKind, SurfaceSessionSnapshot>>;

export function snapshotSurfaceSession(
  cache: SurfaceSessionCache,
  surface: SurfaceKind,
  snap: SurfaceSessionSnapshot,
): SurfaceSessionCache {
  if (!snap.sessionId) {
    const next = { ...cache };
    delete next[surface];
    return next;
  }
  return { ...cache, [surface]: { ...snap, items: snap.items.slice() } };
}

export function restoreSurfaceSession(
  cache: SurfaceSessionCache,
  surface: SurfaceKind,
): SurfaceSessionSnapshot | null {
  return cache[surface] ?? null;
}

/** Engine identity cannot move across surfaces; UI reconnects rather than wiping. */
export function engineCannotShareSessionAcrossSurfaces(): true {
  return true;
}
