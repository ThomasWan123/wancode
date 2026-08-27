import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { WorkDesk } from "./WorkDesk";

const t = STRINGS.en;

describe("WorkDesk", () => {
  it("empty state asks to open a folder, not import a quarantined document", () => {
    render(
      <WorkDesk
        folder=""
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    const region = screen.getByRole("region", { name: t.workDeskTitle });
    expect(region).toBeVisible();
    expect(screen.getAllByRole("button", { name: t.workOpenFolder }).length).toBeGreaterThan(0);
    expect(screen.getByText(t.workDeskEmptyHint)).toBeVisible();
    expect(screen.queryByRole("button", { name: /import document/i })).toBeNull();
    expect(screen.queryByRole("button", { name: t.workAddFile })).toBeNull();
    expect(screen.queryByText(/copied read-only into this session's workspace/i)).toBeNull();
    expect(screen.queryByText(/fingerprint/i)).toBeNull();
    expect(screen.queryByText(/not a PDF editor/i)).toBeNull();
    expect(screen.queryByText(/this is not a PDF editor/i)).toBeNull();
  });

  it("open-folder empty state is the hero CTA", async () => {
    const user = userEvent.setup();
    const onOpenFolder = vi.fn();
    render(
      <WorkDesk
        folder=""
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={onOpenFolder}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    const buttons = screen.getAllByRole("button", { name: t.workOpenFolder });
    expect(buttons.length).toBeGreaterThan(1);
    await user.click(buttons[buttons.length - 1]);
    expect(onOpenFolder).toHaveBeenCalled();
  });

  it("folder-open empty state is a drop target, still without Import-document", () => {
    render(
      <WorkDesk
        folder="D:/client-pack"
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    expect(screen.getByText("client-pack")).toBeVisible();
    expect(screen.getByText(t.workFolderEmpty)).toBeVisible();
    expect(screen.getByRole("button", { name: t.workAddFile })).toBeVisible();
    expect(screen.queryByRole("button", { name: /import document/i })).toBeNull();
  });

  it("lists pdf, docx, xlsx, pptx, and images from the opened folder", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <WorkDesk
        folder="D:/docs"
        files={[
          { path: "brief.pdf", kind: "pdf" },
          { path: "notes.docx", kind: "docx" },
          { path: "budget.xlsx", kind: "xlsx" },
          { path: "deck.pptx", kind: "pptx" },
          { path: "chart.png", kind: "png" },
        ]}
        selectedPath={null}
        onSelect={onSelect}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    expect(screen.getByRole("button", { name: /brief\.pdf/ })).toBeVisible();
    expect(screen.getByRole("button", { name: /notes\.docx/ })).toBeVisible();
    expect(screen.getByRole("button", { name: /budget\.xlsx/ })).toBeVisible();
    expect(screen.getByRole("button", { name: /deck\.pptx/ })).toBeVisible();
    expect(screen.getByRole("button", { name: /chart\.png/ })).toBeVisible();
    await user.click(screen.getByRole("button", { name: /budget\.xlsx/ }));
    expect(onSelect).toHaveBeenCalledWith("budget.xlsx");
  });

  it("renders nested folders in the project rail without a permanent preview pane", () => {
    render(
      <WorkDesk
        folder="D:/docs"
        files={[{ path: "finance/notes.docx", kind: "docx" }]}
        selectedPath="finance/notes.docx"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    expect(screen.getByText("finance")).toBeVisible();
    expect(screen.getByRole("button", { name: "finance/notes.docx" })).toHaveClass("active");
    expect(document.querySelector(".work-desk-preview")).toBeNull();
  });

  it("filters project files without changing the underlying file count", async () => {
    const user = userEvent.setup();
    render(
      <WorkDesk
        folder="D:/docs"
        files={[
          { path: "brief.pdf", kind: "pdf" },
          { path: "finance/budget.xlsx", kind: "xlsx" },
        ]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        t={t}
      />,
    );
    await user.type(screen.getByRole("textbox", { name: t.workSearchFiles }), "budget");
    expect(screen.queryByRole("button", { name: "brief.pdf" })).toBeNull();
    expect(screen.getByRole("button", { name: "finance/budget.xlsx" })).toBeVisible();
    expect(document.querySelector(".work-desk-foot")).toHaveTextContent(t.workFilesCount(2));
  });

  it("drop with a file path places that file into the folder", () => {
    const onDropPaths = vi.fn();
    render(
      <WorkDesk
        folder="D:/docs"
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={onDropPaths}
        t={t}
      />,
    );
    const file = new File(["%PDF"], "brief.pdf", { type: "application/pdf" });
    Object.defineProperty(file, "path", { value: "C:\\downloads\\brief.pdf" });
    fireEvent.drop(screen.getByRole("region", { name: t.workDeskTitle }), {
      dataTransfer: { files: [file] },
    });
    expect(onDropPaths).toHaveBeenCalledWith(["C:\\downloads\\brief.pdf"]);
  });

  it("keeps current-project sessions resumable and searchable", async () => {
    const user = userEvent.setup();
    const onResumeSession = vi.fn();
    const onSearchSessions = vi.fn();
    render(
      <WorkDesk
        folder="D:/docs"
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        sessions={[{
          session_id: "work-1",
          title: "Quarterly analysis",
          updated_at: "2026-08-27T12:30:00Z",
          num_messages: 6,
        }]}
        onResumeSession={onResumeSession}
        onSearchSessions={onSearchSessions}
        t={t}
      />,
    );
    await user.click(screen.getByText("Quarterly analysis"));
    expect(onResumeSession).toHaveBeenCalledWith("work-1");
    fireEvent.change(screen.getByRole("textbox", { name: t.searchPlaceholder }), {
      target: { value: "budget" },
    });
    expect(onSearchSessions).toHaveBeenCalledWith("budget");
  });

  it("keeps current-project sessions renameable and deletable", async () => {
    const user = userEvent.setup();
    const onRenameSession = vi.fn();
    const onDeleteSession = vi.fn();
    vi.spyOn(window, "prompt").mockReturnValue("Renamed analysis");
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const session = {
      session_id: "work-2",
      title: "Original analysis",
      updated_at: "2026-08-27T12:30:00Z",
      num_messages: 4,
    };
    render(
      <WorkDesk
        folder="D:/docs"
        files={[]}
        selectedPath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenFolder={vi.fn()}
        onAddFiles={vi.fn()}
        onDropPaths={vi.fn()}
        sessions={[session]}
        onRenameSession={onRenameSession}
        onDeleteSession={onDeleteSession}
        t={t}
      />,
    );
    await user.click(screen.getByRole("button", { name: `${t.renameSession}: Original analysis` }));
    expect(onRenameSession).toHaveBeenCalledWith(session, "Renamed analysis");
    await user.click(screen.getByRole("button", { name: `${t.deleteSession}: Original analysis` }));
    expect(onDeleteSession).toHaveBeenCalledWith(session);
  });
});
