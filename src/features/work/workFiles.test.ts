import { describe, expect, it } from "vitest";
import {
  canParseForWorkContext,
  fileBaseName,
  folderBaseName,
  isWorkDocument,
  joinWorkspacePath,
  pathsFromDataTransfer,
  workDeskFiles,
  workDocKind,
} from "./workFiles";

describe("workFiles", () => {
  it("treats pdf, docx, and xlsx as ordinary Work documents", () => {
    expect(workDocKind("brief.pdf")).toBe("pdf");
    expect(workDocKind("notes.DOCX")).toBe("docx");
    expect(workDocKind("budget.xlsx")).toBe("xlsx");
    expect(workDocKind("nested/q1/Budget.XLSX")).toBe("xlsx");
    expect(isWorkDocument("readme.md")).toBe(false);
    expect(isWorkDocument("data.csv")).toBe(false);
  });

  it("lists only document files from a folder listing", () => {
    expect(
      workDeskFiles(["src/app.ts", "brief.pdf", "notes.docx", "budget.xlsx", "README.md"]),
    ).toEqual([
      { path: "brief.pdf", kind: "pdf" },
      { path: "notes.docx", kind: "docx" },
      { path: "budget.xlsx", kind: "xlsx" },
    ]);
  });

  it("parses PDF/Word for the existing Work context path, not Excel", () => {
    expect(canParseForWorkContext("pdf")).toBe(true);
    expect(canParseForWorkContext("docx")).toBe(true);
    expect(canParseForWorkContext("xlsx")).toBe(false);
    expect(canParseForWorkContext(null)).toBe(false);
  });

  it("joins a relative file onto a Windows or POSIX folder", () => {
    expect(joinWorkspacePath("D:\\docs", "brief.pdf")).toBe("D:\\docs\\brief.pdf");
    expect(joinWorkspacePath("/home/me/docs", "nested/a.xlsx")).toBe("/home/me/docs/nested/a.xlsx");
    expect(fileBaseName("nested\\q1\\budget.xlsx")).toBe("budget.xlsx");
    expect(folderBaseName("D:\\work\\client-pack\\")).toBe("client-pack");
  });

  it("reads absolute paths off a Tauri/WebView file drop", () => {
    const file = new File(["x"], "brief.pdf");
    Object.defineProperty(file, "path", { value: "C:\\docs\\brief.pdf" });
    expect(pathsFromDataTransfer({ files: [file] })).toEqual(["C:\\docs\\brief.pdf"]);
    expect(pathsFromDataTransfer({ files: [new File(["x"], "no-path.pdf")] })).toEqual([]);
  });
});
