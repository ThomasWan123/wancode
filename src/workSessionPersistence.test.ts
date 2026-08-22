import { describe, expect, it, vi } from "vitest";
import {
  forgetWorkSession,
  forgetWorkWorkspace,
  loadWorkSession,
  loadWorkWorkspace,
  rememberWorkSession,
  rememberWorkWorkspace,
} from "./workSessionPersistence";

function storage(initial: string | null = null) {
  const values = new Map<string, string>();
  if (initial !== null) values.set("wancode-work-session", initial);
  return {
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, next: string) => {
      values.set(key, next);
    }),
    removeItem: vi.fn((key: string) => {
      values.delete(key);
    }),
  };
}

describe("Work session persistence", () => {
  it("round-trips the last durable Work session", () => {
    const target = storage();
    rememberWorkSession(target, " session-42 ");
    expect(loadWorkSession(target)).toBe("session-42");
  });

  it("keeps the durable workspace when only the engine session becomes stale", () => {
    const target = storage();
    rememberWorkWorkspace(target, " ws-123 ");
    expect(loadWorkWorkspace(target)).toBe("ws-123");
    forgetWorkSession(target);
    expect(loadWorkWorkspace(target)).toBe("ws-123");
    forgetWorkWorkspace(target);
    expect(loadWorkWorkspace(target)).toBeUndefined();
  });

  it("ignores blank values and can forget a stale session", () => {
    const target = storage("   ");
    expect(loadWorkSession(target)).toBeUndefined();
    rememberWorkSession(target, "   ");
    expect(target.setItem).not.toHaveBeenCalled();
    forgetWorkSession(target);
    expect(loadWorkSession(target)).toBeUndefined();
  });
});
