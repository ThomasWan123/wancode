import { describe, expect, it } from "vitest";
import { STRINGS } from "./i18n";
import { buildSuggestions } from "./homeSuggestions";

const dirtyRepo = { isRepo: true, files: [{ path: "src/App.tsx" }] };
const files = ["README.md", "src/App.test.tsx"];

describe("surface-specific home suggestions", () => {
  it("keeps repository and test actions in Code", () => {
    const labels = buildSuggestions(files, dirtyRepo, STRINGS.en, "code").map((item) => item.label);

    expect(labels).toContain(STRINGS.en.sugReviewChanges);
    expect(labels).toContain(STRINGS.en.sugCommitMsg);
    expect(labels).not.toContain(STRINGS.en.sugChatAsk);
    expect(labels).not.toContain(STRINGS.en.sugWorkFind);
  });

  it("uses conversational actions in Chat and ignores git/test state", () => {
    const labels = buildSuggestions(files, dirtyRepo, STRINGS.en, "chat").map((item) => item.label);

    expect(labels).toEqual([
      STRINGS.en.sugChatAsk,
      STRINGS.en.sugChatExplain,
      STRINGS.en.sugChatSummarize,
    ]);
    expect(labels).not.toContain(STRINGS.en.sugReviewChanges);
    expect(labels).not.toContain(STRINGS.en.sugRunTests);
  });

  it("uses document actions in Work and ignores git/test state", () => {
    const labels = buildSuggestions(files, dirtyRepo, STRINGS.en, "work").map((item) => item.label);

    expect(labels).toEqual([
      STRINGS.en.sugWorkSummarize,
      STRINGS.en.sugWorkFind,
      STRINGS.en.sugWorkCompare,
    ]);
    expect(labels).not.toContain(STRINGS.en.sugReviewChanges);
    expect(labels).not.toContain(STRINGS.en.sugRunTests);
  });
});
