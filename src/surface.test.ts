import { describe, expect, it } from "vitest";
import {
  decideBackendSurface,
  parseSurface,
  resolveActiveSurface,
  surfaceNeedsWorkspace,
  surfaceSwitchRequiresNewSession,
  WORK_UI_READY,
} from "./surface";

describe("surface navigation contract", () => {
  it("recognizes known surfaces and fails unknown back to Code", () => {
    expect(parseSurface("chat")).toBe("chat");
    expect(parseSurface("code")).toBe("code");
    // W2:Work 现在是一等层(不再坍缩为 Code)。
    expect(parseSurface("work")).toBe("work");
    // 未知/未接线的层(cowork 尚未在前端接线)、null 一律回 Code。
    expect(parseSurface("cowork")).toBe("code");
    expect(parseSurface(null)).toBe("code");
    expect(parseSurface(123)).toBe("code");
  });

  it("requires a new session when changing an active session's layer", () => {
    expect(surfaceSwitchRequiresNewSession("chat", "code", "s1")).toBe(true);
    expect(surfaceSwitchRequiresNewSession("code", "work", "s1")).toBe(true);
    expect(surfaceSwitchRequiresNewSession("chat", "chat", "s1")).toBe(false);
    expect(surfaceSwitchRequiresNewSession("chat", "code", "")).toBe(false);
  });

  it("only Work needs a workspace identity", () => {
    expect(surfaceNeedsWorkspace("work")).toBe(true);
    expect(surfaceNeedsWorkspace("chat")).toBe(false);
    expect(surfaceNeedsWorkspace("code")).toBe(false);
  });

  // codex W2-fe-a R1:激活门 —— Work UI 未就绪前,持久化/后端回传的 Work
  // 不得成为当前层(否则 UI fall through 到 Code、启动被后端拒)。
  it("activation gate keeps Work unreachable until the UI is wired", () => {
    // parseSurface(纯校验)仍认 Work —— 供 W2-fe-b 用。
    expect(parseSurface("work")).toBe("work");
    if (!WORK_UI_READY) {
      // 但可激活层把 Work 降级为 Code(fail-closed)。
      expect(resolveActiveSurface("work")).toBe("code");
    } else {
      expect(resolveActiveSurface("work")).toBe("work");
    }
    // chat/code 不受门影响;未知仍回 Code。
    expect(resolveActiveSurface("chat")).toBe("chat");
    expect(resolveActiveSurface("code")).toBe("code");
    expect(resolveActiveSurface("cowork")).toBe("code");
    expect(resolveActiveSurface(null)).toBe("code");
  });

  // codex W2-fe-a R2:后端已启动会话回传 Work 时,决策必须是 **reject**(不降级
  // 掩盖)。startSession 据此抛错、在 setSessionId 之前——Work session_id 结构上
  // 永不激活。chat/code 则 activate。
  it("backend-returned Work is rejected (not relabeled) until the UI is wired", () => {
    const workDecision = decideBackendSurface("work");
    if (!WORK_UI_READY) {
      expect(workDecision).toEqual({ activate: false, reason: "work-ui-not-ready" });
    } else {
      expect(workDecision).toEqual({ activate: true, surface: "work" });
    }
    expect(decideBackendSurface("chat")).toEqual({ activate: true, surface: "chat" });
    expect(decideBackendSurface("code")).toEqual({ activate: true, surface: "code" });
    // 未知后端值(含未接线的 cowork)按 Code 激活(fail-closed 到已接线层)。
    expect(decideBackendSurface("cowork")).toEqual({ activate: true, surface: "code" });
  });
});
