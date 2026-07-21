/* v0.13 拆分：消息流渲染（用户/助手/思考/提示/工具卡片 + 内联审批 + 全局审批条）。
   步 A 透传。红线：
   - 工具卡片的内联审批只在 permission.toolCallId 能对上卡片时渲染，
     对不上时走底部全局 permission-bar（两处互斥，别同时出现）；
   - DiffView 从 App 层以 prop 传入（依赖 App 内 ToolDiff 类型，避免环形依赖）。 */
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { IconCheck, IconCopy, IconGitBranch } from "../../icons";

export function Messages(props: Record<string, any>) {
  const { DiffView, bottomRef, busy, copiedIdx, copyMessage, error, forkFrom, items, openThoughts, permission, respondPermission, setOpenThoughts, transcriptMode, workspace, t } = props;
  const compact = transcriptMode === "compact";
  const verbose = transcriptMode === "verbose";
  return (
    <>
      <section className="messages" style={items.length === 0 && !busy ? { display: "none" } : undefined}>
        {items.map((it: any, i: any) => {
          if (it.kind === "user")
            return (
              <div key={i} className="msg-wrap user">
                <div className="msg user">{it.text}</div>
                <div className="msg-actions">
                  <button
                    className="icon-btn msg-action"
                    title={t.forkHere}
                    disabled={busy || !workspace}
                    onClick={() => forkFrom(i, it.text)}
                  >
                    <IconGitBranch size={14} />
                  </button>
                  <button
                    className="icon-btn msg-action"
                    title={copiedIdx === i ? t.copied : t.copyMessage}
                    onClick={() => copyMessage(it.text, i)}
                  >
                    {copiedIdx === i ? <IconCheck size={14} /> : <IconCopy size={14} />}
                  </button>
                </div>
              </div>
            );
          if (it.kind === "assistant")
            return (
              <div key={i} className="msg-wrap">
                <div className="msg assistant">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{it.text}</ReactMarkdown>
                </div>
                <div className="msg-actions">
                  <button
                    className="icon-btn msg-action"
                    title={copiedIdx === i ? t.copied : t.copyMessage}
                    onClick={() => copyMessage(it.text, i)}
                  >
                    {copiedIdx === i ? <IconCheck size={14} /> : <IconCopy size={14} />}
                  </button>
                </div>
              </div>
            );
          if (it.kind === "thought") {
            if (compact) return null; // 紧凑档隐藏思考过程
            return (
              <details
                key={i}
                className="msg thought"
                open={verbose || openThoughts.has(i)}
                onToggle={(e) => {
                  const isOpen = (e.currentTarget as HTMLDetailsElement).open;
                  setOpenThoughts((prev: any) => {
                    const next = new Set(prev);
                    if (isOpen) next.add(i);
                    else next.delete(i);
                    return next;
                  });
                }}
              >
                <summary>{t.thinking}</summary>
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{it.text}</ReactMarkdown>
              </details>
            );
          }
          if (it.kind === "note")
            return (
              <div key={i} className="msg note">
                <IconCheck size={13} /> {it.text}
              </div>
            );
          const inlinePerm =
            permission && permission.toolCallId && permission.toolCallId === it.call.toolCallId
              ? permission
              : null;
          return (
            <div key={i} className={`tool-row ${it.call.status ?? ""} ${inlinePerm ? "awaiting" : ""}`}>
              <div className="tool-head">
                <span className="tool-dot" aria-hidden />
                <span className="tool-title">{it.call.title ?? it.call.kind ?? t.toolCall}</span>
              </div>
              {!compact && it.call.diffs.map((d: any, j: any) => (
                <DiffView key={j} diff={d} />
              ))}
              {!compact && it.call.output && (
                <details className="tool-result" open={verbose}>
                  <summary>
                    <span className="elbow" aria-hidden>⎿</span>
                    {t.output}
                  </summary>
                  <pre>{it.call.output}</pre>
                </details>
              )}
              {inlinePerm && (
                <div className="inline-approval">
                  <span className="elbow" aria-hidden>⎿</span>
                  <span className="inline-approval-label">{t.needApproval}</span>
                  {inlinePerm.options.map((o: any) => (
                    <button key={o.optionId} onClick={() => respondPermission(o.optionId)}>
                      {o.name}
                    </button>
                  ))}
                  <button className="deny" onClick={() => respondPermission(null)}>
                    {t.deny}
                  </button>
                </div>
              )}
            </div>
          );
        })}
        {busy && <div className="msg pending">{t.thinkingNow}</div>}
        {error && <div className="msg error">⚠ {error}</div>}
        <div ref={bottomRef} />
      </section>

      {permission &&
        !(
          permission.toolCallId &&
          items.some((it: any) => it.kind === "tool" && it.call.toolCallId === permission.toolCallId)
        ) && (
        <div className="permission-bar">
          <div className="permission-title">🔐 {t.needApproval}{permission.title}</div>
          <div className="permission-actions">
            {permission.options.map((o: any) => (
              <button key={o.optionId} onClick={() => respondPermission(o.optionId)}>
                {o.name}
              </button>
            ))}
            <button className="deny" onClick={() => respondPermission(null)}>
              {t.deny}
            </button>
          </div>
        </div>
      )}
    </>
  );
}
