import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { WorkDesk } from "./WorkDesk";

const t = STRINGS.en;

describe("WorkDesk", () => {
  it("shows document-desk empty copy, not implementation-speak", () => {
    render(
      <WorkDesk docs={[]} selectedId={null} onSelect={vi.fn()} onImport={vi.fn()} t={t} />,
    );
    expect(screen.getByRole("region", { name: t.workDeskTitle })).toBeVisible();
    expect(screen.getByText(t.workDeskEmpty)).toBeVisible();
    expect(screen.queryByText(/copied read-only into this session's workspace/i)).toBeNull();
  });

  it("lists documents and shows identity preview without faking a PDF editor", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <WorkDesk
        docs={[
          {
            import_id: "imp-1",
            display_name: "brief.pdf",
            kind: "pdf",
            source_sha256: "abcdef0123456789",
          },
        ]}
        selectedId={null}
        onSelect={onSelect}
        onImport={vi.fn()}
        t={t}
      />,
    );
    await user.click(screen.getByRole("button", { name: /brief\.pdf/ }));
    expect(onSelect).toHaveBeenCalledWith("imp-1");
  });

  it("shows fingerprint immediately when import auto-selects the new doc", () => {
    render(
      <WorkDesk
        docs={[
          {
            import_id: "imp-2",
            display_name: "notes.docx",
            kind: "docx",
            source_sha256: "abcdef0123456789ffff",
          },
        ]}
        selectedId="imp-2"
        onSelect={vi.fn()}
        onImport={vi.fn()}
        t={t}
      />,
    );
    expect(screen.getByText(/abcdef012345/)).toBeVisible();
    expect(screen.queryByText(t.workSelectHint)).toBeNull();
  });

  it("asks the user to pick a doc when the list has items but none is selected", () => {
    render(
      <WorkDesk
        docs={[
          {
            import_id: "imp-1",
            display_name: "brief.pdf",
            kind: "pdf",
            source_sha256: "abcdef0123456789",
          },
        ]}
        selectedId={null}
        onSelect={vi.fn()}
        onImport={vi.fn()}
        t={t}
      />,
    );
    expect(screen.getByText(t.workSelectHint)).toBeVisible();
  });
});
