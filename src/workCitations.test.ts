import { describe, expect, it } from "vitest";
import {
  attachWorkCitationChecks,
  verifyWorkCitations,
  type WorkCitationSource,
} from "./workCitations";

const sources: WorkCitationSource[] = [
  { documentName: "report.docx", blockPath: "body/p[41]" },
  { documentName: "brief.pdf", blockPath: "page[3]/chunk[0]" },
  { documentName: "budget.xlsx", blockPath: "workbook/sheet[1:Budget]/cell[B7]" },
  { documentName: "deck.pptx", blockPath: "slides/slide[2]/text[4]" },
];

describe("verifyWorkCitations", () => {
  it("recognizes and verifies all four supported document path families", () => {
    const text = sources.map((source) => `[${source.documentName} — ${source.blockPath}]`).join("\n");
    const checks = verifyWorkCitations(text, sources);
    expect(checks).toHaveLength(4);
    expect(checks.every((check) => check.status === "verified")).toBe(true);
  });

  it("accepts a legal closing bracket in the document name", () => {
    const bracketed = [{ documentName: "Q1] report.docx", blockPath: "body/p[2]" }];
    expect(verifyWorkCitations("[Q1] report.docx — body/p[2]]", bracketed)[0]).toMatchObject({
      documentName: "Q1] report.docx",
      status: "verified",
    });
  });

  it("marks invented documents and block paths as unverifiable", () => {
    const checks = verifyWorkCitations(
      "[ghost.docx — body/p[41]] [report.docx — body/p[999]]",
      sources,
    );
    expect(checks.map((check) => [check.status, check.reason])).toEqual([
      ["unverifiable", "missing"],
      ["unverifiable", "missing"],
    ]);
  });

  it("fails closed when the same visible document/path pair is ambiguous", () => {
    const duplicate = [...sources, { ...sources[0] }];
    expect(verifyWorkCitations("[report.docx — body/p[41]]", duplicate)[0]).toMatchObject({
      status: "unverifiable",
      reason: "ambiguous",
    });
  });

  it("does not misclassify ordinary bracketed prose as a source citation", () => {
    expect(verifyWorkCitations("Revenue changed [2025 — 2026].", sources)).toEqual([]);
    expect(verifyWorkCitations("`[report.docx - body/p[41]]`", sources)).toEqual([]);
  });
});

describe("attachWorkCitationChecks", () => {
  it("decorates only the latest assistant reply and is idempotent across event/response paths", () => {
    type Item = { kind: string; text?: string; citationChecks?: ReturnType<typeof verifyWorkCitations> };
    const earlier: Item = { kind: "assistant", text: "old [ghost.docx — body/p[1]]" };
    const latest: Item = { kind: "assistant", text: "new [report.docx — body/p[41]]" };
    const once = attachWorkCitationChecks([earlier, { kind: "tool" }, latest], sources);
    expect(once[0].citationChecks).toBeUndefined();
    expect(once[2].citationChecks?.[0].status).toBe("verified");
    expect(attachWorkCitationChecks(once, [])).toBe(once);
  });

  it("does not attach a completion catalog to an older reply when the turn has no final assistant", () => {
    const items = [
      { kind: "assistant", text: "old [report.docx — body/p[41]]" },
      { kind: "user", text: "new prompt" },
      { kind: "tool" },
    ];
    expect(attachWorkCitationChecks(items, sources)).toBe(items);
  });
});
