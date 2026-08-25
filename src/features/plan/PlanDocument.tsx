/* Plan as a first-class annotatable document, not a chat-bubble modal.
   Engine stays in read-only plan mode until Approve. Escape = request changes. */
import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { IconCheck, IconPencil } from "../../icons";

function composeFeedback(note: string, draft: string, original: string): string | null {
  const body = draft !== original ? draft : "";
  const parts = [note.trim(), body && body !== note.trim() ? body : ""].filter(Boolean);
  return parts.length ? parts.join("\n\n") : null;
}

export function PlanDocument(props: Record<string, any>) {
  const { planApproval, planFeedback, setPlanFeedback, respondPlan, t } = props;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    if (!planApproval) return;
    setDraft(planApproval.planContent ?? "");
    setEditing(false);
  }, [planApproval?.id, planApproval?.planContent]);

  useEffect(() => {
    if (!planApproval) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (editing) {
        setEditing(false);
        return;
      }
      const original = planApproval.planContent ?? "";
      respondPlan("cancelled", composeFeedback(planFeedback, draft, original));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [planApproval, editing, draft, planFeedback, respondPlan]);

  if (!planApproval) return null;

  const original = planApproval.planContent ?? "";

  function requestChanges() {
    const feedback = composeFeedback(planFeedback, draft, original);
    setPlanFeedback(feedback ?? "");
    respondPlan("cancelled", feedback);
  }

  function approve() {
    respondPlan("approved", composeFeedback(planFeedback, draft, original));
  }

  return (
    <section className="plan-doc" role="region" aria-label={t.planDocTitle}>
      <header className="plan-doc-head">
        <div>
          <div className="plan-doc-title">{t.planDocTitle}</div>
          <div className="plan-doc-hint">{t.planKeepReadonly}</div>
        </div>
        <button
          className="ghost small"
          onClick={() => setEditing((v) => !v)}
          title={editing ? t.planDocPreview : t.planDocEdit}
        >
          {editing ? <IconCheck size={13} /> : <IconPencil size={13} />}
          {editing ? t.planDocPreview : t.planDocEdit}
        </button>
      </header>
      {editing ? (
        <textarea
          className="plan-doc-editor"
          value={draft}
          onChange={(e) => setDraft(e.currentTarget.value)}
          spellCheck={false}
        />
      ) : (
        <div className="plan-doc-body">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>
            {draft || "_(empty plan)_"}
          </ReactMarkdown>
        </div>
      )}
      <label className="plan-doc-comments-label" htmlFor="plan-doc-comments">
        {t.planDocComments}
      </label>
      <textarea
        id="plan-doc-comments"
        className="plan-feedback"
        value={planFeedback}
        placeholder={t.planFeedbackPlaceholder}
        onChange={(e) => setPlanFeedback(e.currentTarget.value)}
        rows={3}
      />
      <div className="plan-approval-actions">
        <button onClick={approve}>{t.planApprove}</button>
        <button className="ghost" data-dialog-autofocus onClick={requestChanges}>
          {t.planRequestChanges}
        </button>
        <button className="deny" onClick={() => respondPlan("abandoned")}>
          {t.planAbandon}
        </button>
      </div>
    </section>
  );
}
