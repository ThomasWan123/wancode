import { describe, expect, it } from "vitest";
import { isWorkImageKind, WORK_DOCUMENT_EXTENSIONS } from "./workFormats";

describe("Work document formats", () => {
  it("keeps the picker aligned with the modern parser and image matrix", () => {
    expect(WORK_DOCUMENT_EXTENSIONS).toEqual([
      "pdf",
      "docx",
      "xlsx",
      "pptx",
      "png",
      "jpg",
      "jpeg",
      "webp",
    ]);
    expect(WORK_DOCUMENT_EXTENSIONS).not.toContain("doc");
    expect(WORK_DOCUMENT_EXTENSIONS).not.toContain("xls");
    expect(WORK_DOCUMENT_EXTENSIONS).not.toContain("ppt");
  });

  it("identifies persisted Work images for the model capability gate", () => {
    expect(["png", "jpeg", "jpg", "webp"].every(isWorkImageKind)).toBe(true);
    expect(["pdf", "docx", "xlsx", "pptx"].some(isWorkImageKind)).toBe(false);
  });
});
