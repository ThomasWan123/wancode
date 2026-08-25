import { describe, expect, it } from "vitest";
import { detectComposerPopup, popupIsVisible } from "./composerPopup";

describe("popupIsVisible", () => {
  it("does not block send when the popup is missing or has zero rows", () => {
    expect(popupIsVisible(null, 0)).toBe(false);
    expect(popupIsVisible({ kind: "slash", query: "/nope", sel: 0 }, 0)).toBe(false);
    expect(popupIsVisible({ kind: "at", query: "", sel: 0 }, 0)).toBe(false);
  });

  it("blocks send only while a visible row can be chosen", () => {
    expect(popupIsVisible({ kind: "slash", query: "/", sel: 0 }, 3)).toBe(true);
    expect(popupIsVisible({ kind: "at", query: "src", sel: 0 }, 1)).toBe(true);
  });
});

describe("detectComposerPopup", () => {
  it("does not treat a normal sentence with spaces as a popup", () => {
    const v = "hello world test";
    expect(detectComposerPopup(v, v.length)).toBeNull();
    expect(detectComposerPopup("Reply with exactly EMP-OK", 25)).toBeNull();
  });

  it("detects a leading slash token and an @ mention after whitespace", () => {
    expect(detectComposerPopup("/rev", 4)).toEqual({ kind: "slash", query: "/rev" });
    expect(detectComposerPopup("see @src/app", 12)).toEqual({ kind: "at", query: "src/app" });
  });
});
