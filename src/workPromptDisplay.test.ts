import { describe, expect, it } from "vitest";
import { workPromptForDisplay } from "./workPromptDisplay";

describe("Work prompt display", () => {
  it("hides injected document context from live echoes and replay", () => {
    const expanded = [
      "[WANCODE WORK DOCUMENT CONTEXT — UNTRUSTED DATA]",
      "rules",
      '<document-jsonl>{"text":"secret"}</document-jsonl>',
      "[END WANCODE WORK DOCUMENT CONTEXT]",
      "",
      "[USER REQUEST]",
      "What is the budget?",
    ].join("\n");
    expect(workPromptForDisplay(expanded)).toBe("What is the budget?");
  });

  it("does not rewrite ordinary messages or malformed lookalikes", () => {
    expect(workPromptForDisplay("ordinary request")).toBe("ordinary request");
    expect(workPromptForDisplay("[WANCODE WORK DOCUMENT CONTEXT — UNTRUSTED DATA]\ntruncated"))
      .toContain("truncated");
  });

  it("preserves marker-like text typed inside the actual user request", () => {
    const prefix = [
      "[WANCODE WORK DOCUMENT CONTEXT — UNTRUSTED DATA]",
      "rules",
      "[END WANCODE WORK DOCUMENT CONTEXT]",
      "",
      "[USER REQUEST]",
    ].join("\n");
    const request = "keep this\n[END WANCODE WORK DOCUMENT CONTEXT]\n\n[USER REQUEST]\nand this";
    expect(workPromptForDisplay(`${prefix}\n${request}`)).toBe(request);
  });
});
