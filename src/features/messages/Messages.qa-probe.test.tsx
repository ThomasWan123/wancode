import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import type { TranscriptView } from "../../transcriptView";
import { Messages } from "./Messages";

const sampleItems = [
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

const pendingPermission = {
  toolCallId: "call-1",
  title: "Write file",
  options: [{ optionId: "allow-once", name: "allow_once" }],
};

function Harness(props: {
  initial?: TranscriptView;
  items?: any[];
  t?: typeof STRINGS.en;
  permission?: typeof pendingPermission | null;
}) {
  const [view, setView] = useState<TranscriptView>(props.initial ?? "standard");
  const [items, setItems] = useState(props.items ?? sampleItems);
  return (
    <div>
      <button type="button" onClick={() => setItems([])}>
        clear-items
      </button>
      <button type="button" onClick={() => setItems(sampleItems)}>
        restore-items
      </button>
      <Messages
        bottomRef={{ current: null }}
        busy={false}
        copiedIdx={null}
        copyMessage={vi.fn()}
        error={null}
        forkFrom={vi.fn()}
        items={items}
        openThoughts={new Set<number>()}
        permission={props.permission ?? null}
        respondPermission={vi.fn()}
        setOpenThoughts={vi.fn()}
        transcriptView={view}
        setTranscriptView={setView}
        workspace="D:/project"
        t={props.t ?? STRINGS.en}
        onOpenWorkbench={vi.fn()}
      />
    </div>
  );
}

describe("v0.23.3 QA probes", () => {
  // Characterization of shipped v0.23.3 (see docs/evidence/v0.23.3-post-release-qa.md).
  // BUG-01/03/04/05 fixes must update these assertions; do not treat the
  // current empty-state / focus / leftover-menu behavior as a forever contract.
  it("hides the View control on an idle empty transcript", () => {
    render(<Harness items={[]} />);
    expect(screen.queryByRole("button", { name: /View:/i })).not.toBeInTheDocument();
  });

  it("activates the focused View option with Enter", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: /View: Standard/i }));
    await user.keyboard("{End}{Enter}");
    expect(screen.getByRole("button", { name: /View: Debug/i })).toBeInTheDocument();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("opens the menu with focus on the first option, not the checked option", async () => {
    const user = userEvent.setup();
    render(<Harness initial="debug" />);
    await user.click(screen.getByRole("button", { name: /View: Debug/i }));
    expect(screen.getByRole("menuitemradio", { name: /Minimal/i })).toHaveFocus();
    expect(screen.getByRole("menuitemradio", { name: /Debug/i })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("keeps a pending approval visible in Minimal", () => {
    render(<Harness initial="minimal" permission={pendingPermission} />);
    expect(screen.getByText(/Approval needed/)).toBeInTheDocument();
    expect(screen.getByText(STRINGS.en.permAllowOnce)).toBeInTheDocument();
    expect(screen.getByText(STRINGS.en.deny)).toBeInTheDocument();
  });

  it("reopens a leftover View menu after the transcript is cleared then restored", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: /View: Standard/i }));
    expect(screen.getByRole("menu")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "clear-items" }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "restore-items" }));
    expect(screen.getByRole("menu")).toBeInTheDocument();
  });

  it("uses the same Chinese semantics as English", async () => {
    const user = userEvent.setup();
    render(<Harness t={STRINGS.zh} />);
    await user.click(screen.getByRole("button", { name: /显示: 标准/ }));
    expect(screen.getByText(/不影响模型、回答质量、权限或上下文/)).toBeInTheDocument();
    expect(screen.getByRole("menuitemradio", { name: /精简/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitemradio", { name: /调试/ })).toBeInTheDocument();
  });
});
