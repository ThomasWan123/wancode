import { describe, expect, it } from "vitest";
import { parseSurface, surfaceSwitchRequiresNewSession } from "./surface";

describe("surface navigation contract", () => {
  it("fails unknown navigation state back to Code", () => {
    expect(parseSurface("chat")).toBe("chat");
    expect(parseSurface("work")).toBe("code");
    expect(parseSurface(null)).toBe("code");
  });

  it("requires a new session when changing an active session's layer", () => {
    expect(surfaceSwitchRequiresNewSession("chat", "code", "s1")).toBe(true);
    expect(surfaceSwitchRequiresNewSession("chat", "chat", "s1")).toBe(false);
    expect(surfaceSwitchRequiresNewSession("chat", "code", "")).toBe(false);
  });
});
