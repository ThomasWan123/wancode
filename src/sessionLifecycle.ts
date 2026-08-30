import type { SurfaceKind } from "./surface";

export type SessionActionEntry = { session_id: string };

type StartSession = (
  resume?: string,
  workspace?: string,
  keepReplayCap?: boolean,
  workWorkspaceId?: string,
) => Promise<string>;

export type SessionLifecycleDeps = {
  getActiveSessionId: () => string;
  getRuntimeWorkspace: () => string;
  getDisplayWorkspace: () => string;
  getSurface: () => SurfaceKind;
  getWorkWorkspaceId: () => string;
  startSession: StartSession;
  invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>;
  refreshSessions: (workspace: string) => void | Promise<void>;
  clearActiveSession: () => void;
};

/**
 * Session-list operations run against the engine session's real cwd. Work's
 * visible folder is a source folder and is deliberately not a session cwd.
 */
export function createSessionLifecycle(deps: SessionLifecycleDeps) {
  const workId = () =>
    deps.getSurface() === "work" ? deps.getWorkWorkspaceId() || undefined : undefined;
  const actionWorkspace = () =>
    deps.getRuntimeWorkspace() || deps.getDisplayWorkspace();

  async function ensureEngineForTarget(targetSessionId: string): Promise<string> {
    const active = deps.getActiveSessionId();
    if (active) return active;
    const resumed = await deps.startSession(
      targetSessionId,
      undefined,
      false,
      workId(),
    );
    if (!resumed) throw new Error("SESSION_ACTION_START_FAILED");
    return resumed;
  }

  return {
    newWorkSession(): Promise<string> {
      return deps.startSession(undefined, undefined, false, workId());
    },

    async search(query: string): Promise<unknown> {
      if (!deps.getActiveSessionId()) {
        const started = await deps.startSession(undefined, undefined, false, workId());
        if (!started) throw new Error("SESSION_SEARCH_START_FAILED");
      }
      return deps.invoke("agent_session_search", {
        query,
        workspace: actionWorkspace(),
      });
    },

    async rename(entry: SessionActionEntry, title: string): Promise<void> {
      await ensureEngineForTarget(entry.session_id);
      await deps.invoke("agent_session_rename", {
        sessionId: entry.session_id,
        title,
        workspace: actionWorkspace(),
      });
      await deps.refreshSessions(actionWorkspace());
    },

    async remove(entry: SessionActionEntry): Promise<void> {
      await ensureEngineForTarget(entry.session_id);
      await deps.invoke("agent_session_delete", {
        sessionId: entry.session_id,
        workspace: actionWorkspace(),
      });
      if (entry.session_id === deps.getActiveSessionId()) {
        deps.clearActiveSession();
      }
      await deps.refreshSessions(actionWorkspace());
    },
  };
}
