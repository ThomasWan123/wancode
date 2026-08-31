import { describe, expect, it, vi } from "vitest";
import { observePrimaryTurn } from "./turnLifecycle";

describe("primary turn completion", () => {
  it("releases the composer when the command succeeds even without a turn-end event", async () => {
    const settled = vi.fn();
    observePrimaryTurn(Promise.resolve(), vi.fn(), settled);
    await Promise.resolve();
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
  });

  it("reports failures and still releases the composer exactly once", async () => {
    const error = vi.fn();
    const settled = vi.fn();
    observePrimaryTurn(Promise.reject(new Error("engine closed")), error, settled);
    await Promise.resolve();
    await Promise.resolve();
    expect(error).toHaveBeenCalledTimes(1);
    expect(settled).toHaveBeenCalledTimes(1);
  });
});
