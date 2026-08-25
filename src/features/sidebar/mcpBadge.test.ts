import { describe, expect, it } from "vitest";
import { liveMcpBadgeCount } from "./mcpBadge";

describe("liveMcpBadgeCount", () => {
  it("does not fall back to configured servers when this session loaded none", () => {
    expect(liveMcpBadgeCount([])).toBe(0);
    expect(liveMcpBadgeCount(null)).toBe(0);
  });

  it("counts only live servers that are not session-disabled", () => {
    expect(
      liveMcpBadgeCount([
        { session: { enabled: true } },
        { session: { enabled: true } },
        { session: { enabled: false } },
      ]),
    ).toBe(2);
  });
});
