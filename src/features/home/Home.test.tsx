import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { Home } from "./Home";

describe("Home suggestions", () => {
  it("passes the active surface into the suggestion policy", () => {
    const buildSuggestions = vi.fn(() => [
      { label: STRINGS.en.sugWorkFind, prompt: STRINGS.en.sugWorkFindP },
    ]);

    render(
      <Home
        buildSuggestions={buildSuggestions}
        baseName={(path: string) => path}
        fileList={["budget.xlsx"]}
        gitInfo={{ isRepo: true, files: [{ path: "src/App.tsx" }] }}
        items={[]}
        busy={false}
        onComposerChange={vi.fn()}
        otherRecent={[]}
        planSteps={[]}
        sessionId="work-session"
        setInput={vi.fn()}
        startSession={vi.fn()}
        surface="work"
        taRef={{ current: null }}
        t={STRINGS.en}
        planPending={false}
      />,
    );

    expect(buildSuggestions).toHaveBeenCalledWith(
      ["budget.xlsx"],
      { isRepo: true, files: [{ path: "src/App.tsx" }] },
      STRINGS.en,
      "work",
    );
    expect(screen.getByRole("button", { name: STRINGS.en.sugWorkFind })).toBeInTheDocument();
    expect(screen.queryByText(STRINGS.en.sugReviewChanges)).not.toBeInTheDocument();
  });
});
