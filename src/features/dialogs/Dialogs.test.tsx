import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));

import { STRINGS } from "../../i18n";
import { Dialogs } from "./Dialogs";

function renderDialogs(overrides: Record<string, unknown>) {
  const props: Record<string, unknown> = {
    answers: {},
    editingSkill: null,
    planApproval: null,
    planFeedback: "",
    question: null,
    rewindPoints: null,
    trustReq: null,
    setError: vi.fn(),
    setTrustReq: vi.fn(),
    respondPlan: vi.fn(),
    respondQuestion: vi.fn(),
    t: STRINGS.en,
    ...overrides,
  };
  render(<Dialogs {...props} />);
  return props;
}

describe("blocking dialog cancellation", () => {
  beforeEach(() => invokeMock.mockReset().mockResolvedValue(undefined));

  it("focuses the fail-closed trust action and Escape sends a rejection", async () => {
    const user = userEvent.setup();
    const setTrustReq = vi.fn();
    renderDialogs({
      setTrustReq,
      trustReq: { id: "trust-1", workspace: "D:/repo", configKinds: [] },
    });

    expect(screen.getByRole("dialog", { name: STRINGS.en.trustTitle })).toBeVisible();
    expect(screen.getByRole("button", { name: STRINGS.en.trustNo })).toHaveFocus();
    await user.keyboard("{Escape}");

    expect(setTrustReq).toHaveBeenCalledWith(null);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("agent_trust_respond", {
        id: "trust-1",
        trust: false,
      }),
    );
  });

  it("routes question Escape through the real cancel response", async () => {
    const user = userEvent.setup();
    const respondQuestion = vi.fn();
    renderDialogs({
      respondQuestion,
      question: { questions: [{ question: "Choose", options: [] }] },
    });

    expect(screen.getByRole("button", { name: STRINGS.en.cancel })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(respondQuestion).toHaveBeenCalledWith(false);
  });

  it("routes plan Escape through the request-changes response", async () => {
    const user = userEvent.setup();
    const respondPlan = vi.fn();
    renderDialogs({
      respondPlan,
      planApproval: { planContent: "Plan" },
    });

    expect(screen.getByRole("button", { name: STRINGS.en.planRequestChanges })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(respondPlan).toHaveBeenCalledWith("cancelled");
  });
});
