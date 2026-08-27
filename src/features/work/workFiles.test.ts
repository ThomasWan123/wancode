import { describe, expect, it } from "vitest";
import {
  appendWorkMention,
  canParseForWorkContext,
  fileBaseName,
  folderBaseName,
  isLegacyOffice,
  isWorkDocument,
  joinWorkspacePath,
  pathsFromDataTransfer,
  referencedWorkSources,
  sourceIsWorkImage,
  workDeskFiles,
  workDocKind,
} from "./workFiles";

describe("workFiles", () => {
  it("treats pdf, docx, xlsx, pptx, and modern images as ordinary Work documents", () => {
    expect(workDocKind("brief.pdf")).toBe("pdf");
    expect(workDocKind("notes.DOCX")).toBe("docx");
    expect(workDocKind("budget.xlsx")).toBe("xlsx");
    expect(workDocKind("nested/q1/Budget.XLSX")).toBe("xlsx");
    expect(workDocKind("deck.pptx")).toBe("pptx");
    expect(workDocKind("chart.PNG")).toBe("png");
    expect(workDocKind("photo.jpg")).toBe("jpg");
    expect(workDocKind("photo.jpeg")).toBe("jpeg");
    expect(workDocKind("diagram.webp")).toBe("webp");
    expect(isWorkDocument("readme.md")).toBe(false);
    expect(isWorkDocument("data.csv")).toBe(false);
    expect(isLegacyOffice("legacy.doc")).toBe(true);
    expect(isLegacyOffice("legacy.xls")).toBe(true);
    expect(isLegacyOffice("legacy.ppt")).toBe(true);
    expect(isLegacyOffice("budget.xlsx")).toBe(false);
  });

  it("lists only document files from a folder listing", () => {
    expect(
      workDeskFiles([
        "src/app.ts",
        "brief.pdf",
        "notes.docx",
        "budget.xlsx",
        "deck.pptx",
        "chart.png",
        "README.md",
      ]),
    ).toEqual([
      { path: "brief.pdf", kind: "pdf" },
      { path: "notes.docx", kind: "docx" },
      { path: "budget.xlsx", kind: "xlsx" },
      { path: "deck.pptx", kind: "pptx" },
      { path: "chart.png", kind: "png" },
    ]);
  });

  it("parses office documents at send; images take the vision path", () => {
    expect(canParseForWorkContext("pdf")).toBe(true);
    expect(canParseForWorkContext("docx")).toBe(true);
    expect(canParseForWorkContext("xlsx")).toBe(true);
    expect(canParseForWorkContext("pptx")).toBe(true);
    expect(canParseForWorkContext("png")).toBe(false);
    expect(canParseForWorkContext(null)).toBe(false);
    expect(sourceIsWorkImage("chart.png")).toBe(true);
    expect(sourceIsWorkImage("budget.xlsx")).toBe(false);
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

  it("snapshots @mentioned or selected folder files, not the whole desk", () => {
    const files = [
      { path: "brief.pdf", kind: "pdf" as const },
      { path: "budget.xlsx", kind: "xlsx" as const },
      { path: "chart.png", kind: "png" as const },
    ];
    expect(
      referencedWorkSources({
        text: "Summarize @budget.xlsx please",
        folder: "D:/docs",
        files,
        selectedPath: "brief.pdf",
      }),
    ).toEqual({ sources: ["D:/docs/budget.xlsx"], issue: null });
    expect(
      referencedWorkSources({
        text: "What is this?",
        folder: "D:/docs",
        files,
        selectedPath: "chart.png",
      }),
    ).toEqual({ sources: ["D:/docs/chart.png"], issue: null });
    expect(
      referencedWorkSources({
        text: "Hello",
        folder: "D:/docs",
        files,
        selectedPath: null,
      }),
    ).toEqual({ sources: [], issue: null });
  });

  it("fails closed for unresolved or ambiguous mentions", () => {
    const files = [
      { path: "a/budget.xlsx", kind: "xlsx" as const },
      { path: "b/budget.xlsx", kind: "xlsx" as const },
      { path: "brief.pdf", kind: "pdf" as const },
    ];
    expect(
      referencedWorkSources({
        text: "Use @missing.pdf",
        folder: "D:/docs",
        files,
        selectedPath: "brief.pdf",
      }),
    ).toEqual({ sources: [], issue: "unresolved" });
    expect(
      referencedWorkSources({
        text: "Use @budget.xlsx",
        folder: "D:/docs",
        files,
        selectedPath: "brief.pdf",
      }),
    ).toEqual({ sources: [], issue: "ambiguous" });
    expect(
      referencedWorkSources({
        text: "Use @a/budget.xlsx and @b/budget.xlsx",
        folder: "D:/docs",
        files,
        selectedPath: null,
      }),
    ).toEqual({
      sources: ["D:/docs/a/budget.xlsx", "D:/docs/b/budget.xlsx"],
      issue: null,
    });
  });

  it("does not reuse a selected path that is absent from the current folder", () => {
    expect(
      referencedWorkSources({
        text: "Summarize the selection",
        folder: "D:/new-folder",
        files: [{ path: "new.pdf", kind: "pdf" }],
        selectedPath: "old.pdf",
      }),
    ).toEqual({ sources: [], issue: null });
  });

  it("inserts exact file references once without path-prefix collisions", () => {
    expect(appendWorkMention("Review", "brief.pdf")).toBe("Review @brief.pdf ");
    expect(appendWorkMention("Review @brief.pdf ", "brief.pdf")).toBe("Review @brief.pdf ");
    expect(appendWorkMention("Review @nested/brief.pdf ", "brief.pdf")).toBe(
      "Review @nested/brief.pdf @brief.pdf ",
    );
  });
});
