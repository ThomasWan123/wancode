import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { Workbench } from "./Workbench";

const t = STRINGS.en;

function renderWb(overrides: Record<string, any> = {}) {
  const props: Record<string, any> = {
    showWorkbench: true,
    setShowWorkbench: vi.fn(),
    wbTab: "diff",
    setWbTab: vi.fn(),
    wbFiles: null,
    wbLoading: false,
    wbOpenPaths: new Set(),
    setWbOpenPaths: vi.fn(),
    refreshWorkbench: vi.fn(),
    gitOp: vi.fn(),
    fileList: [],
    gitInfo: { isRepo: false },
    reviewResult: null,
    reviewLoading: false,
    runReview: vi.fn(),
    fixFindings: vi.fn(),
    previewUrl: "",
    setPreviewUrl: vi.fn(),
    previewLive: null,
    setPreviewLive: vi.fn(),
    t,
    ...overrides,
  };
  return { ...render(<Workbench {...props} />), props };
}

describe("Workbench git empty states", () => {
  it("Diff uses the friendly need-repo copy when the workspace is not git", () => {
    renderWb({ wbTab: "diff", gitInfo: { isRepo: false }, wbFiles: null });
    expect(screen.getByText(t.gitNeedRepo)).toBeVisible();
    expect(screen.queryByText(t.gitNotRepo)).toBeNull();
    expect(screen.queryByText(t.gitClean)).toBeNull();
  });

  it("Review hides the run CTA and shows the same copy when git is unavailable", () => {
    renderWb({ wbTab: "review", gitInfo: { isRepo: false } });
    expect(screen.getByText(t.gitNeedRepo)).toBeVisible();
    expect(screen.queryByRole("button", { name: t.reviewRun })).toBeNull();
  });

  it("keeps gitClean for an actual clean repo", () => {
    renderWb({ wbTab: "diff", gitInfo: { isRepo: true }, wbFiles: [] });
    expect(screen.getByText(t.gitClean)).toBeVisible();
    expect(screen.queryByText(t.gitNeedRepo)).toBeNull();
  });

  it("Review still offers the run button in a real repo", () => {
    renderWb({ wbTab: "review", gitInfo: { isRepo: true, files: [] } });
    expect(screen.getByRole("button", { name: t.reviewRun })).toBeEnabled();
    expect(screen.queryByText(t.gitNeedRepo)).toBeNull();
  });
});
