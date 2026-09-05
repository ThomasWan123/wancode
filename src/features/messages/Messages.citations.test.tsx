import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { Messages } from "./Messages";

function renderMessages(items: any[]) {
  return render(
    <Messages
      bottomRef={{ current: null }}
      busy={false}
      copiedIdx={null}
      copyMessage={vi.fn()}
      error={null}
      forkFrom={vi.fn()}
      items={items}
      openThoughts={new Set<number>()}
      permission={null}
      respondPermission={vi.fn()}
      setOpenThoughts={vi.fn()}
      transcriptView="standard"
      setTranscriptView={vi.fn()}
      workspace="D:/project"
      t={STRINGS.en}
      onOpenWorkbench={vi.fn()}
    />,
  );
}

describe("Work citation status", () => {
  it("renders verified and unverifiable citations with distinct labels", async () => {
    const { container } = renderMessages([
      {
        kind: "assistant",
        text: "Answer [report.docx — body/p[1]] [ghost.pdf — page[9]/chunk[0]]",
        citationChecks: [
          {
            citation: "[report.docx — body/p[1]]",
            documentName: "report.docx",
            blockPath: "body/p[1]",
            status: "verified",
          },
          {
            citation: "[ghost.pdf — page[9]/chunk[0]]",
            documentName: "ghost.pdf",
            blockPath: "page[9]/chunk[0]",
            status: "unverifiable",
            reason: "missing",
          },
        ],
      },
    ]);

    expect(await screen.findByLabelText("Source verification")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.getByText("Unverifiable")).toBeInTheDocument();
    expect(container.querySelectorAll(".citation-check.verified")).toHaveLength(1);
    expect(container.querySelectorAll(".citation-check.unverifiable")).toHaveLength(1);
  });

  it("does not claim verification when the answer has no citation checks", () => {
    renderMessages([{ kind: "assistant", text: "No source cited." }]);
    expect(screen.queryByLabelText("Source verification")).not.toBeInTheDocument();
  });
});
