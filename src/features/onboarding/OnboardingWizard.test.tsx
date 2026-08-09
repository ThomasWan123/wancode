import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { STRINGS } from "../../i18n";
import { OnboardingWizard } from "./OnboardingWizard";

function renderWizard(onClose = vi.fn()) {
  render(
    <button>Background action</button>,
  );
  render(
    <OnboardingWizard
      t={STRINGS.en}
      onConfigured={vi.fn()}
      onOpenFolder={vi.fn()}
      onCustomEndpoint={vi.fn()}
      onClose={onClose}
    />,
  );
  return onClose;
}

describe("OnboardingWizard accessibility", () => {
  it("opens as a named modal and moves focus into the dialog", () => {
    renderWizard();

    const dialog = screen.getByRole("dialog", { name: "Welcome to WanCode" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByRole("button", { name: /GLM Coding Plan/ })).toHaveFocus();
  });

  it("cycles Tab focus within the dialog instead of reaching background controls", async () => {
    const user = userEvent.setup();
    renderWizard();

    const first = screen.getByRole("button", { name: /GLM Coding Plan/ });
    const last = screen.getByRole("button", { name: "Later" });
    expect(first).toHaveFocus();

    await user.tab({ shift: true });
    expect(last).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
    expect(screen.getByRole("button", { name: "Background action" })).not.toHaveFocus();
  });

  it("supports Escape dismissal", async () => {
    const user = userEvent.setup();
    const onClose = renderWizard();

    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
