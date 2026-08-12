import { describe, expect, it } from "vitest";
import {
  parseSurface,
  surfaceNeedsWorkspace,
  surfaceSwitchRequiresNewSession,
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
});
