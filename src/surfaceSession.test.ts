import { describe, expect, it } from "vitest";
import {
  engineCannotShareSessionAcrossSurfaces,
  restoreSurfaceSession,
  snapshotSurfaceSession,
  type SurfaceSessionCache,
} from "./surfaceSession";

describe("surface session cache", () => {
  it("keeps a transcript snapshot per surface without sharing engine identity", () => {
    expect(engineCannotShareSessionAcrossSurfaces()).toBe(true);
    let cache: SurfaceSessionCache = {};
    cache = snapshotSurfaceSession(cache, "code", {
      sessionId: "code-1",
      items: [{ kind: "user", text: "hi" }],
      workspace: "D:/proj",
      workWorkspaceId: "",
    });
    cache = snapshotSurfaceSession(cache, "chat", {
      sessionId: "chat-1",
      items: [{ kind: "assistant", text: "ok" }],
      workspace: "",
      workWorkspaceId: "",
    });
    expect(restoreSurfaceSession(cache, "code")?.items).toEqual([{ kind: "user", text: "hi" }]);
    expect(restoreSurfaceSession(cache, "chat")?.sessionId).toBe("chat-1");
    expect(restoreSurfaceSession(cache, "work")).toBeNull();
  });

  it("clears a surface snapshot when the session id is empty", () => {
    let cache = snapshotSurfaceSession({}, "code", {
      sessionId: "code-1",
      items: [],
      workspace: "",
      workWorkspaceId: "",
    });
    cache = snapshotSurfaceSession(cache, "code", {
      sessionId: "",
      items: [],
      workspace: "",
      workWorkspaceId: "",
    });
    expect(restoreSurfaceSession(cache, "code")).toBeNull();
  });
});
