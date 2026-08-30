import { describe, expect, it, vi } from "vitest";
import { createSessionLifecycle } from "./sessionLifecycle";

function fixture(surface: "chat" | "code" | "work" = "work") {
  const state = {
    active: "live-1",
    runtimeWorkspace: "C:/app/work/ws-real",
    displayWorkspace: "D:/fixture",
    workWorkspaceId: "ws-durable",
    surface,
  };
  const invoke = vi.fn(async (_command: string, _args: Record<string, unknown>) => ({
    result: { results: [] },
  }));
  const startSession = vi.fn(async (resume?: string) => {
    state.active = resume || "live-new";
    return state.active;
  });
  const refreshSessions = vi.fn(async () => undefined);
  const clearActiveSession = vi.fn(() => { state.active = ""; });
  const lifecycle = createSessionLifecycle({
    getActiveSessionId: () => state.active,
    getRuntimeWorkspace: () => state.runtimeWorkspace,
    getDisplayWorkspace: () => state.displayWorkspace,
    getSurface: () => state.surface,
    getWorkWorkspaceId: () => state.workWorkspaceId,
    startSession,
    invoke,
    refreshSessions,
    clearActiveSession,
  });
  return { state, lifecycle, invoke, startSession, refreshSessions, clearActiveSession };
}

describe("session lifecycle", () => {
  it("creates a real Work session under the existing durable document workspace", async () => {
    const f = fixture();
    await f.lifecycle.newWorkSession();
    expect(f.startSession).toHaveBeenCalledWith(undefined, undefined, false, "ws-durable");
  });

  it("renames with the live runtime cwd, never the visible Work source folder", async () => {
    const f = fixture();
    await f.lifecycle.rename({ session_id: "stored-2" }, "Renamed");
    expect(f.invoke).toHaveBeenCalledWith("agent_session_rename", {
      sessionId: "stored-2",
      title: "Renamed",
      workspace: "C:/app/work/ws-real",
    });
    expect(f.refreshSessions).toHaveBeenCalledWith("C:/app/work/ws-real");
    expect(f.startSession).not.toHaveBeenCalled();
  });

  it("resumes the target instead of creating an unrelated blank session when no engine is live", async () => {
    const f = fixture();
    f.state.active = "";
    await f.lifecycle.rename({ session_id: "stored-2" }, "Renamed");
    expect(f.startSession).toHaveBeenCalledWith("stored-2", undefined, false, "ws-durable");
    expect(f.invoke).toHaveBeenCalledWith(
      "agent_session_rename",
      expect.objectContaining({ sessionId: "stored-2" }),
    );
  });

  it("deletes a non-current session without clearing the live transcript", async () => {
    const f = fixture();
    await f.lifecycle.remove({ session_id: "stored-2" });
    expect(f.clearActiveSession).not.toHaveBeenCalled();
    expect(f.invoke).toHaveBeenCalledWith(
      "agent_session_delete",
      expect.objectContaining({ sessionId: "stored-2", workspace: "C:/app/work/ws-real" }),
    );
  });

  it("clears UI identity only after the current session was successfully deleted", async () => {
    const f = fixture();
    await f.lifecycle.remove({ session_id: "live-1" });
    expect(f.clearActiveSession).toHaveBeenCalledTimes(1);
  });

  it("keeps the current session intact when delete fails", async () => {
    const f = fixture();
    f.invoke.mockRejectedValueOnce(new Error("backend down"));
    await expect(f.lifecycle.remove({ session_id: "live-1" })).rejects.toThrow("backend down");
    expect(f.state.active).toBe("live-1");
    expect(f.clearActiveSession).not.toHaveBeenCalled();
    expect(f.refreshSessions).not.toHaveBeenCalled();
  });

  it("searches the runtime cwd and starts within the existing Work identity when needed", async () => {
    const f = fixture();
    f.state.active = "";
    await f.lifecycle.search("budget");
    expect(f.startSession).toHaveBeenCalledWith(undefined, undefined, false, "ws-durable");
    expect(f.invoke).toHaveBeenCalledWith("agent_session_search", {
      query: "budget",
      workspace: "C:/app/work/ws-real",
    });
  });

  it("does not leak Work identity into Chat or Code target-session recovery", async () => {
    for (const surface of ["chat", "code"] as const) {
      const f = fixture(surface);
      f.state.active = "";
      await f.lifecycle.rename({ session_id: `${surface}-stored` }, "Renamed");
      expect(f.startSession).toHaveBeenCalledWith(
        `${surface}-stored`,
        undefined,
        false,
        undefined,
      );
    }
  });

  it("survives 50 heavy-user rename/delete cycles without wrong-cwd or ghost starts", async () => {
    const f = fixture();
    for (let i = 0; i < 50; i++) {
      await f.lifecycle.rename({ session_id: `stored-${i}` }, `Title ${i}`);
      await f.lifecycle.remove({ session_id: `old-${i}` });
    }
    expect(f.startSession).not.toHaveBeenCalled();
    expect(f.invoke).toHaveBeenCalledTimes(100);
    for (const [, args] of f.invoke.mock.calls) {
      expect(args.workspace).toBe("C:/app/work/ws-real");
      expect(args.workspace).not.toBe("D:/fixture");
    }
  });
});
