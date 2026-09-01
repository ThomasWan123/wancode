import { describe, expect, it, vi } from "vitest";
import { observePrimaryTurn } from "./turnLifecycle";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, reject, resolve };
}

describe("primary turn completion", () => {
  it("releases the composer when the command succeeds even without a turn-end event", async () => {
    const settled = vi.fn();
    observePrimaryTurn(Promise.resolve(), () => true, vi.fn(), settled);
    await Promise.resolve();
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
  });

  it("reports failures and still releases the composer exactly once", async () => {
    const error = vi.fn();
    const settled = vi.fn();
    observePrimaryTurn(Promise.reject(new Error("engine closed")), () => true, error, settled);
    await Promise.resolve();
    await Promise.resolve();
    expect(error).toHaveBeenCalledTimes(1);
    expect(settled).toHaveBeenCalledTimes(1);
  });

  it("does not let turn A settle or report errors after turn B becomes current", async () => {
    let currentGeneration = 1;
    const turnA = deferred();
    const settledA = vi.fn();
    const errorA = vi.fn();
    observePrimaryTurn(turnA.promise, () => currentGeneration === 1, errorA, settledA);

    currentGeneration = 2;
    const turnB = deferred();
    const settledB = vi.fn();
    observePrimaryTurn(turnB.promise, () => currentGeneration === 2, vi.fn(), settledB);

    // Before this fix, turn-end unlocked the composer before A's invoke
    // resolved. A's late finally must not unlock the already-running B.
    turnA.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(settledA).not.toHaveBeenCalled();
    expect(errorA).not.toHaveBeenCalled();
    expect(settledB).not.toHaveBeenCalled();

    turnB.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(settledB).toHaveBeenCalledTimes(1);

    // Reverse ordering: even after the current B has ended, a still-later A
    // rejection cannot replace the terminal state or report a stale error.
    currentGeneration = 3;
    const lateA = deferred();
    observePrimaryTurn(lateA.promise, () => currentGeneration === 3, errorA, settledA);
    currentGeneration = 4;
    const laterB = deferred();
    const laterBSettled = vi.fn();
    observePrimaryTurn(laterB.promise, () => currentGeneration === 4, vi.fn(), laterBSettled);
    laterB.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(laterBSettled).toHaveBeenCalledTimes(1);

    lateA.reject(new Error("stale A failure"));
    await Promise.resolve();
    await Promise.resolve();
    expect(errorA).not.toHaveBeenCalled();
    expect(settledA).not.toHaveBeenCalled();
  });
});
