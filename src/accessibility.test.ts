import { describe, expect, it, vi } from "vitest";
import { activateOnKeyboard } from "./accessibility";

describe("activateOnKeyboard", () => {
  it.each(["Enter", " "])("activates on %j", (key) => {
    const preventDefault = vi.fn();
    const action = vi.fn();

    activateOnKeyboard({ key, preventDefault }, action);

    expect(preventDefault).toHaveBeenCalledOnce();
    expect(action).toHaveBeenCalledOnce();
  });

  it("ignores unrelated keys", () => {
    const preventDefault = vi.fn();
    const action = vi.fn();

    activateOnKeyboard({ key: "ArrowDown", preventDefault }, action);

    expect(preventDefault).not.toHaveBeenCalled();
    expect(action).not.toHaveBeenCalled();
  });
});
