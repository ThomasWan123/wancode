import { describe, expect, it, vi } from "vitest";
import { createLatestStartCoordinator } from "./startCoordinator";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

describe("latest start coordinator", () => {
  it("deduplicates StrictMode-style duplicate starts", async () => {
    const gate = deferred<string>();
    const task = vi.fn(async () => gate.promise);
    const coordinator = createLatestStartCoordinator("");
    const first = coordinator.schedule("work:new", task);
    const duplicate = coordinator.schedule("work:new", task);
    expect(duplicate).toBe(first);
    gate.resolve("session-1");
    await expect(first).resolves.toBe("session-1");
    expect(task).toHaveBeenCalledTimes(1);
  });

  it("serializes starts, skips superseded queued intents, and marks an active result stale", async () => {
    const active = deferred<string>();
    const started = deferred<void>();
    const seen: string[] = [];
    let firstStillCurrent = true;
    const coordinator = createLatestStartCoordinator("");
    const first = coordinator.schedule("work:new", async (isCurrent) => {
      seen.push("first");
      started.resolve();
      const value = await active.promise;
      firstStillCurrent = isCurrent();
      return isCurrent() ? value : "";
    });
    // Let the first intent enter its backend call before a newer surface wins.
    await started.promise;
    const middle = coordinator.schedule("chat:a", async () => {
      seen.push("middle");
      return "middle";
    });
    const latest = coordinator.schedule("chat:b", async () => {
      seen.push("latest");
      return "session-b";
    });
    active.resolve("session-work");

    await expect(first).resolves.toBe("");
    await expect(middle).resolves.toBe("");
    await expect(latest).resolves.toBe("session-b");
    expect(firstStillCurrent).toBe(false);
    expect(seen).toEqual(["first", "latest"]);
  });
});
