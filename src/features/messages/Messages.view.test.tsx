import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import type { TranscriptView } from "../../transcriptView";
import { Messages } from "./Messages";

const items = [
  { kind: "thought", text: "A provider-visible process summary" },
  {
    kind: "tool",
    call: {
      toolCallId: "call-1",
      title: "Updated settings",
      status: "completed",
      output: "tool diagnostics",
      diffs: [{ oldText: "before", newText: "after" }],
    },
  },
];

function Harness({ initial = "standard", initialItems = items }: { initial?: TranscriptView; initialItems?: any[] }) {
  const [view, setView] = useState<TranscriptView>(initial);
  const [visibleItems, setVisibleItems] = useState(initialItems);
  return (
    <>
      <button type="button" onClick={() => setVisibleItems([])}>clear-items</button>
      <button type="button" onClick={() => setVisibleItems(items)}>restore-items</button>
      <Messages
        bottomRef={{ current: null }}
        busy={false}
        copiedIdx={null}
        copyMessage={vi.fn()}
        error={null}
        forkFrom={vi.fn()}
        items={visibleItems}
        openThoughts={new Set<number>()}
        permission={null}
        respondPermission={vi.fn()}
        setOpenThoughts={vi.fn()}
        transcriptView={view}
        setTranscriptView={setView}
        workspace="D:/project"
        t={STRINGS.en}
        onOpenWorkbench={vi.fn()}
      />
    </>
  );
}

describe("message display preference", () => {
  it("keeps display controls out of the composer and explains their scope", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: /View: Standard/i }));
    expect(screen.getByText(/Changes display only/i)).toBeInTheDocument();
    expect(screen.getByRole("menuitemradio", { name: /Standard/i })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("supports menu keyboard navigation and escape", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const trigger = screen.getByRole("button", { name: /View: Standard/i });
    await user.click(trigger);
    expect(screen.getByRole("menuitemradio", { name: /Standard/i })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitemradio", { name: /Debug/i })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("menuitemradio", { name: /Minimal/i })).toHaveFocus();
    await user.keyboard("{End}");
    expect(screen.getByRole("menuitemradio", { name: /Debug/i })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("keeps the display control reachable on an idle empty transcript", () => {
    const { container } = render(<Harness initialItems={[]} />);

    expect(screen.getByRole("button", { name: /View: Standard/i })).toBeInTheDocument();
    expect(container.querySelector(".messages-empty")).toBeInTheDocument();
  });

  it("opens the menu with focus on the checked display option", async () => {
    const user = userEvent.setup();
    render(<Harness initial="debug" />);

    await user.click(screen.getByRole("button", { name: /View: Debug/i }));
    expect(screen.getByRole("menuitemradio", { name: /Debug/i })).toHaveFocus();
    expect(screen.getByRole("menuitemradio", { name: /Debug/i })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("closes an open display menu when the transcript is cleared", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: /View: Standard/i }));
    expect(screen.getByRole("menu")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "clear-items" }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "restore-items" }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("minimal hides process, review, and tool detail but keeps tool status", () => {
    const { container } = render(<Harness initial="minimal" />);
    expect(screen.queryByText("Process summary")).not.toBeInTheDocument();
    expect(container.querySelector(".review-chip")).not.toBeInTheDocument();
    expect(screen.queryByText("tool diagnostics")).not.toBeInTheDocument();
    expect(screen.getByText("Updated settings")).toBeInTheDocument();
  });

  it("debug expands available process and tool detail", async () => {
    const user = userEvent.setup();
    const { container } = render(<Harness />);

    await user.click(screen.getByRole("button", { name: /View: Standard/i }));
    await user.click(screen.getByRole("menuitemradio", { name: /Debug/i }));

    expect(screen.getByRole("button", { name: /View: Debug/i })).toBeInTheDocument();
    const details = Array.from(container.querySelectorAll("details"));
    expect(details).toHaveLength(2);
    expect(details.every((detail) => detail.open)).toBe(true);
    expect(screen.getByText("tool diagnostics")).toBeInTheDocument();
  });
});
