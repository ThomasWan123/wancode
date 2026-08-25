import { describe, expect, it } from "vitest";
import {
  decideBackendSurface,
  parseSurface,
  resolveActiveSurface,
  surfaceLabel,
  surfaceNeedsWorkspace,
  surfaceSwitchRequiresNewSession,
  WORK_UI_READY,
} from "./surface";
import { STRINGS } from "./i18n";

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

  it("localizes switcher labels through i18n (default zh)", () => {
    expect(surfaceLabel("chat", STRINGS.zh)).toBe("聊天");
    expect(surfaceLabel("code", STRINGS.zh)).toBe("代码");
    expect(surfaceLabel("work", STRINGS.zh)).toBe("工作");
    expect(surfaceLabel("chat", STRINGS.en)).toBe("Chat");
  });

  it("only Work needs a workspace identity", () => {
    expect(surfaceNeedsWorkspace("work")).toBe(true);
    expect(surfaceNeedsWorkspace("chat")).toBe(false);
    expect(surfaceNeedsWorkspace("code")).toBe(false);
  });

  // 激活门(W2-fe-a R1 引入,W2-fe-b 起 Work 已接线放行)。门的形状随
  // WORK_UI_READY:ready → Work 可激活;未 ready → 降级 Code。Cowork/未知恒 Code。
  it("activation gate follows WORK_UI_READY; Cowork/unknown always Code", () => {
    expect(parseSurface("work")).toBe("work"); // 纯校验恒认 Work
    expect(resolveActiveSurface("work")).toBe(WORK_UI_READY ? "work" : "code");
    // chat/code 不受门影响;Cowork(未接线)、未知一律回 Code。
    expect(resolveActiveSurface("chat")).toBe("chat");
    expect(resolveActiveSurface("code")).toBe("code");
    expect(resolveActiveSurface("cowork")).toBe("code");
    expect(resolveActiveSurface(null)).toBe("code");
  });

  // 后端已启动会话回传 surface 的决策(W2-fe-a R2/R3)。W2-fe-b 起 Work 已接线
  // → activate;Cowork 仍未接线 → reject(不降级掩盖身份);畸形 → reject。
  it("backend decision: Work activates when wired, Cowork/unknown rejected", () => {
    expect(decideBackendSurface("work")).toEqual(
      WORK_UI_READY
        ? { activate: true, surface: "work" }
        : { activate: false, reason: "layer-not-wired" },
    );
    // Cowork 始终未接线(Cowork 线未落地)。
    expect(decideBackendSurface("cowork")).toEqual({ activate: false, reason: "layer-not-wired" });
    // 畸形/未知输入 → reject(不猜测、不激活)。
    expect(decideBackendSurface("garbage")).toEqual({ activate: false, reason: "unknown-surface" });
    expect(decideBackendSurface(null)).toEqual({ activate: false, reason: "unknown-surface" });
    // 已接线层照常激活。
    expect(decideBackendSurface("chat")).toEqual({ activate: true, surface: "chat" });
    expect(decideBackendSurface("code")).toEqual({ activate: true, surface: "code" });
  });
});
