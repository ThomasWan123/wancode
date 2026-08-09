import { describe, expect, it, vi } from "vitest";
import { resolveCrashRecovery } from "./crashRecovery";

const info = { sessionId: "session-1", workspace: "D:/repo" };

describe("resolveCrashRecovery", () => {
  it("restores without acknowledging first and closes only after a successful start", async () => {
    const startSession = vi.fn().mockResolvedValue("session-1");
    const acknowledge = vi.fn();

    await expect(
      resolveCrashRecovery("restore", info, { startSession, acknowledge }),
    ).resolves.toBe(true);

    expect(startSession).toHaveBeenCalledWith("session-1", "D:/repo");
    expect(acknowledge).not.toHaveBeenCalled();
  });

  it("keeps the recovery pointer when session restore fails", async () => {
    const startSession = vi.fn().mockResolvedValue("");
    const acknowledge = vi.fn();

    await expect(
      resolveCrashRecovery("restore", info, { startSession, acknowledge }),
    ).resolves.toBe(false);

    expect(acknowledge).not.toHaveBeenCalled();
  });

  it("closes dismiss only after acknowledgement succeeds", async () => {
    const startSession = vi.fn();
    const acknowledge = vi.fn().mockRejectedValue(new Error("disk full"));

    await expect(
      resolveCrashRecovery("dismiss", info, { startSession, acknowledge }),
    ).rejects.toThrow("disk full");

    expect(startSession).not.toHaveBeenCalled();
  });
});
