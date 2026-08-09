import { describe, expect, it } from "vitest";

import { displaySessionTitle } from "./i18n";

describe("displaySessionTitle", () => {
  it.each([undefined, null, "", "(未命名会话)", "(untitled session)"])(
    "uses the active locale for an empty or legacy placeholder title (%s)",
    (title) => {
      expect(displaySessionTitle(title, "(untitled session)")).toBe("(untitled session)");
    },
  );

  it("preserves a real session title", () => {
    expect(displaySessionTitle("Fix login race", "(untitled session)")).toBe("Fix login race");
  });
});
