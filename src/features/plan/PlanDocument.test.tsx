import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { STRINGS } from "../../i18n";
import { PlanDocument } from "./PlanDocument";

const t = STRINGS.en;

describe("PlanDocument", () => {
  it("renders as a document panel with approve / request / abandon", async () => {
    const user = userEvent.setup();
    const respondPlan = vi.fn();
    const setPlanFeedback = vi.fn();
    render(
      <PlanDocument
        t={t}
        planApproval={{ id: 1, planContent: "Ship dark mode" }}
        planFeedback=""
        setPlanFeedback={setPlanFeedback}
        respondPlan={respondPlan}
      />,
    );

    expect(screen.getByRole("region", { name: t.planDocTitle })).toBeVisible();
    expect(screen.getByText("Ship dark mode")).toBeVisible();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText("📋")).toBeNull();

    await user.click(screen.getByRole("button", { name: t.planApprove }));
    expect(respondPlan).toHaveBeenCalledWith("approved", null);
  });

  it("sends request-changes with comments and does not use a modal", async () => {
    const user = userEvent.setup();
    const respondPlan = vi.fn();
    render(
      <PlanDocument
        t={t}
        planApproval={{ id: 2, planContent: "Plan A" }}
        planFeedback="please drop step 3"
        setPlanFeedback={vi.fn()}
        respondPlan={respondPlan}
      />,
    );
    await user.click(screen.getByRole("button", { name: t.planRequestChanges }));
    expect(respondPlan).toHaveBeenCalledWith("cancelled", "please drop step 3");
    expect(document.querySelector(".modal-mask")).toBeNull();
  });

  it("sends the edited markdown on approve so the document is the source of truth", async () => {
    const user = userEvent.setup();
    const respondPlan = vi.fn();
    render(
      <PlanDocument
        t={t}
        planApproval={{ id: 3, planContent: "Plan A" }}
        planFeedback=""
        setPlanFeedback={vi.fn()}
        respondPlan={respondPlan}
      />,
    );
    await user.click(screen.getByRole("button", { name: t.planDocEdit }));
    const editor = document.querySelector(".plan-doc-editor") as HTMLTextAreaElement;
    await user.clear(editor);
    await user.type(editor, "Plan B");
    await user.click(screen.getByRole("button", { name: t.planApprove }));
    expect(respondPlan).toHaveBeenCalledWith("approved", "Plan B");
  });
});
