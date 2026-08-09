import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ModalDialog } from "./ModalDialog";

describe("ModalDialog", () => {
  it("is named, modal, initially focused, and traps focus in both directions", async () => {
    const user = userEvent.setup();
    render(
      <>
        <button>Background</button>
        <ModalDialog ariaLabel="Safety decision">
          <button>Accept</button>
          <button>Reject</button>
        </ModalDialog>
      </>,
    );

    expect(screen.getByRole("dialog", { name: "Safety decision" })).toHaveAttribute(
      "aria-modal",
      "true",
    );
    const accept = screen.getByRole("button", { name: "Accept" });
    const reject = screen.getByRole("button", { name: "Reject" });
    expect(accept).toHaveFocus();

    await user.tab({ shift: true });
    expect(reject).toHaveFocus();
    await user.tab();
    expect(accept).toHaveFocus();
    expect(screen.getByRole("button", { name: "Background" })).not.toHaveFocus();
  });

  it("honors the safe autofocus marker and routes Escape through the caller", async () => {
    const user = userEvent.setup();
    const onEscape = vi.fn();
    render(
      <ModalDialog ariaLabel="Trust folder" onEscape={onEscape}>
        <button>Trust</button>
        <button data-dialog-autofocus>Do not trust</button>
      </ModalDialog>,
    );

    expect(screen.getByRole("button", { name: "Do not trust" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(onEscape).toHaveBeenCalledTimes(1);
  });
});
