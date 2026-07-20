/* v0.13 拆分：全局对话框（回滚/技能编辑/文件夹信任/引擎提问/计划审批）。
   步 A 透传。红线：
   - 信任/提问/计划审批都是引擎在等回包的请求——关闭路径也必须应答
     （respondQuestion(false)/trust:false/respondPlan），不能只 setState 关掉；
   - 回滚 doRewind 的两段式（预览冲突→确认）逻辑留在 App 层。 */
import { invoke } from "@tauri-apps/api/core";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { IconCheck } from "../../icons";

export function Dialogs(props: Record<string, any>) {
  const { answers, doRewind, editingSkill, planApproval, planFeedback, question, refreshSkills, respondPlan, respondQuestion, rewindMode, rewindPoints, setEditingSkill, setError, setPlanFeedback, setRewindMode, setRewindPoints, setTrustReq, toggleAnswer, trustReq, t } = props;
  return (
    <>
      {rewindPoints && (
        <div className="modal-mask" onClick={() => setRewindPoints(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.rewindTitle}</div>
            <div className="modal-section">
              <div className="modal-label">{t.rewindWhat}</div>
              <select value={rewindMode} onChange={(e) => setRewindMode(e.currentTarget.value)}>
                <option value="all">{t.rewindAll}</option>
                <option value="conversation_only">{t.rewindConversation}</option>
                <option value="files_only">{t.rewindFiles}</option>
              </select>
            </div>
            <div className="rewind-list">
              {rewindPoints.length === 0 && (
                <div className="sidebar-empty">{t.noCheckpoints}</div>
              )}
              {rewindPoints.map((p: any) => {
                const idx = p.promptIndex ?? p.prompt_index;
                const files = p.numFileSnapshots ?? p.num_file_snapshots ?? 0;
                return (
                  <div key={idx} className="session-item" onClick={() => doRewind(idx)}>
                    <div className="session-title">
                      #{idx} {p.promptPreview ?? p.prompt_preview ?? t.noPreview}
                    </div>
                    <div className="session-meta">
                      {(p.createdAt ?? p.created_at ?? "").slice(0, 19).replace("T", " ")} · {files} {t.fileSnapshots}
                    </div>
                  </div>
                );
              })}
            </div>
            <button className="ghost" onClick={() => setRewindPoints(null)}>
              {t.cancel}
            </button>
          </div>
        </div>
      )}

      {editingSkill && (
        <div className="modal-mask" onClick={() => setEditingSkill(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()} style={{ width: 620 }}>
            <div className="modal-title">{t.skillEditTitle} · {editingSkill.name}</div>
            <textarea
              className="skill-editor"
              value={editingSkill.content}
              onChange={(e) => setEditingSkill({ ...editingSkill, content: e.currentTarget.value })}
              spellCheck={false}
            />
            <div className="modal-footer">
              <span />
              <div style={{ display: "flex", gap: 8 }}>
                <button className="ghost" onClick={() => setEditingSkill(null)}>{t.cancel}</button>
                <button
                  onClick={async () => {
                    try {
                      await invoke("skill_write", { path: editingSkill.path, content: editingSkill.content });
                      setEditingSkill(null);
                      refreshSkills();
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t.save}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 文件夹信任：这个仓库自带 MCP/hooks/LSP 配置，未授权前引擎已挡住它们。 */}
      {trustReq && (
        <div className="modal-mask">
          <div className="modal trust-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.trustTitle}</div>
            <div className="trust-path">{trustReq.workspace || trustReq.cwd}</div>
            <div className="trust-body">
              {t.trustBody}
              {trustReq.configKinds.length > 0 && (
                <div className="trust-kinds">
                  {trustReq.configKinds.map((k: any) => (
                    <span key={k} className="trust-kind">
                      {k}
                    </span>
                  ))}
                </div>
              )}
            </div>
            <div className="question-actions">
              <button
                onClick={async () => {
                  const id = trustReq.id;
                  setTrustReq(null);
                  await invoke("agent_trust_respond", { id, trust: true }).catch((e) =>
                    setError(String(e)),
                  );
                }}
              >
                {t.trustYes}
              </button>
              <button
                className="ghost"
                onClick={async () => {
                  const id = trustReq.id;
                  setTrustReq(null);
                  await invoke("agent_trust_respond", { id, trust: false }).catch((e) =>
                    setError(String(e)),
                  );
                }}
              >
                {t.trustNo}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 引擎主动提问。之前这个请求被兜底应答成空对象，用户根本看不到。 */}
      {question && (
        <div className="modal-mask">
          <div className="modal question-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.questionTitle}</div>
            <div className="question-body">
              {question.questions.map((q: any) => (
                <div key={q.question} className="question-block">
                  <div className="question-text">{q.question}</div>
                  {q.multiSelect && <div className="question-multi">{t.questionMulti}</div>}
                  <div className="question-options">
                    {(q.options ?? []).map((o: any) => {
                      const picked = (answers[q.question] ?? []).includes(o.label);
                      return (
                        <button
                          key={o.label}
                          className={`question-option ${picked ? "picked" : ""}`}
                          onClick={() => toggleAnswer(q.question, o.label, !!q.multiSelect)}
                        >
                          <span className="question-option-label">
                            {picked && <IconCheck size={13} />}
                            {o.label}
                          </span>
                          {o.description && (
                            <span className="question-option-desc">{o.description}</span>
                          )}
                        </button>
                      );
                    })}
                  </div>
                </div>
              ))}
            </div>
            <div className="question-actions">
              <button
                disabled={Object.keys(answers).length === 0}
                onClick={() => respondQuestion(true)}
              >
                {t.questionSubmit}
              </button>
              <button className="ghost" onClick={() => respondQuestion(false)}>
                {t.cancel}
              </button>
            </div>
          </div>
        </div>
      )}

      {planApproval && (
        <div className="modal-mask">
          <div className="modal plan-approval-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.planApprovalTitle}</div>
            <div className="plan-approval-body">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{planApproval.planContent || "_(empty plan)_"}</ReactMarkdown>
            </div>
            <textarea
              className="plan-feedback"
              value={planFeedback}
              placeholder={t.planFeedbackPlaceholder}
              onChange={(e) => setPlanFeedback(e.currentTarget.value)}
              rows={2}
            />
            <div className="plan-approval-actions">
              <button onClick={() => respondPlan("approved")}>{t.planApprove}</button>
              <button className="ghost" onClick={() => respondPlan("cancelled")}>
                {t.planRequestChanges}
              </button>
              <button className="deny" onClick={() => respondPlan("abandoned")}>
                {t.planAbandon}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
