/* v0.13 拆分：消息流渲染（用户/助手/思考/提示/工具卡片 + 内联审批 + 全局审批条）。
   P0: quiet transcript by default (collapsed outcome chips), review gate instead of naive DiffView,
   thinking as one-line duration stub, SVG icons replace emoji. */
import { useState, useCallback, useEffect } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { IconCheck, IconCopy, IconGitBranch, IconShield } from "../../icons";

function escapeHtml(code: string): string {
  return code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function humanPermLabel(o: { name?: string; kind?: string }, t: any): string {
  const token = `${o.kind ?? ""} ${o.name ?? ""}`.toLowerCase().replace(/[\s-]+/g, "_");
  if (/allow_always|always_allow/.test(token)) return t.permAllowAlways;
  if (/allow_once/.test(token)) return t.permAllowOnce;
  if (/reject_always|deny_always/.test(token)) return t.permRejectAlways;
  if (/reject_once|deny_once/.test(token)) return t.permRejectOnce;
  const name = (o.name ?? "").trim();
  if (/^[a-z]+_[a-z]+$/i.test(name)) return name.replace(/_/g, " ");
  return name || t.approve;
}

function CodeBlock({ className, children, t }: { className?: string; children: string; t: any }) {
  const [copied, setCopied] = useState(false);
  const lang = className?.replace("language-", "") ?? "";
  const code = String(children).replace(/\n$/, "");
  const [highlighted, setHighlighted] = useState(() => escapeHtml(code));

  useEffect(() => {
    let cancelled = false;
    import("../../highlight").then(({ highlightCode }) => {
      if (!cancelled) setHighlighted(highlightCode(code, lang));
    }).catch(() => {
      if (!cancelled) setHighlighted(escapeHtml(code));
    });
    return () => { cancelled = true; };
  }, [code, lang]);

  const onCopy = useCallback(() => {
    navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }, [code]);

  return (
    <div className="code-fence-wrap">
      <div className="code-fence-head">
        <span>{lang || "code"}</span>
        <button className="code-fence-copy" onClick={onCopy} type="button">
          {copied ? <><IconCheck size={12} /> {t.copiedCode}</> : <><IconCopy size={12} /> {t.copyCode}</>}
        </button>
      </div>
      <pre><code className="hljs" dangerouslySetInnerHTML={{ __html: highlighted }} /></pre>
    </div>
  );
}

function ReviewChip({ diffs, onReview, t }: { diffs: any[]; onReview: () => void; t: any }) {
  const fileCount = diffs.length;
  let added = 0, removed = 0;
  for (const d of diffs) {
    const newLines = (d.newText ?? "").split("\n").length;
    const oldLines = (d.oldText ?? "").split("\n").length;
    added += newLines;
    removed += d.oldText !== undefined ? oldLines : 0;
  }
  return (
    <div
      className="review-chip"
      onClick={onReview}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onReview();
        }
      }}
      role="button"
      tabIndex={0}
    >
      <span className="rc-label">{t.reviewChanged(fileCount)}</span>
      <span className="rc-stats">
        <span className="add">+{added}</span>{" "}
        <span className="del">-{removed}</span>
      </span>
      <span className="rc-action">{t.reviewAction} &rsaquo;</span>
    </div>
  );
}

export function Messages(props: Record<string, any>) {
  const { bottomRef, busy, copiedIdx, copyMessage, error, forkFrom, items, openThoughts, permission, respondPermission, setOpenThoughts, transcriptMode, workspace, t, onOpenWorkbench } = props;
  const compact = transcriptMode === "compact";
  const verbose = transcriptMode === "verbose";
  const quiet = transcriptMode === "default" || !transcriptMode;
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
                  <ReactMarkdown
                    remarkPlugins={[remarkGfm]}
                    components={{
                      code({ className, children, ...rest }: any) {
                        const isBlock = rest.node?.position?.start?.line !== rest.node?.position?.end?.line
                          || String(children).includes("\n");
                        if (isBlock) {
                          return <CodeBlock className={className} t={t}>{String(children)}</CodeBlock>;
                        }
                        return <code className={className}>{children}</code>;
                      },
                      pre({ children }: any) {
                        return <>{children}</>;
                      },
                    }}
                  >{it.text}</ReactMarkdown>
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
            if (compact) return null;
            const isQuietCollapsed = quiet && !openThoughts.has(i);
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
                <summary>{isQuietCollapsed ? t.thinkingNow : t.thinking}</summary>
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
          /* Tool call rendering */
          const inlinePerm =
            permission && permission.toolCallId && permission.toolCallId === it.call.toolCallId
              ? permission
              : null;

          const hasDiffs = it.call.diffs && it.call.diffs.length > 0;

          return (
            <div key={i} className={`tool-row ${it.call.status ?? ""} ${inlinePerm ? "awaiting" : ""}`}>
              <div className="tool-head">
                <span className="tool-dot" aria-hidden />
                <span className="tool-title">{it.call.title ?? it.call.kind ?? t.toolCall}</span>
              </div>
              {/* P0-4: Review gate — show summary chip instead of full naive diff */}
              {hasDiffs && !compact && (
                <ReviewChip diffs={it.call.diffs} onReview={() => onOpenWorkbench?.()} t={t} />
              )}
              {!compact && it.call.output && (
                <details className="tool-result" open={verbose}>
                  <summary>
                    <span className="elbow" aria-hidden>&#x23BF;</span>
                    {t.output}
                  </summary>
                  <pre>{it.call.output}</pre>
                </details>
              )}
              {inlinePerm && (
                <div className="inline-approval">
                  <span className="elbow" aria-hidden>&#x23BF;</span>
                  <IconShield size={13} />
                  <span className="inline-approval-label">{t.needApproval}</span>
                  {inlinePerm.options.map((o: any) => (
                    <button key={o.optionId} onClick={() => respondPermission(o.optionId)}>
                      {humanPermLabel(o, t)}
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
        {error && <div className="msg error">{error}</div>}
        <div ref={bottomRef} />
      </section>

      {permission &&
        !(
          permission.toolCallId &&
          items.some((it: any) => it.kind === "tool" && it.call.toolCallId === permission.toolCallId)
        ) && (
        <div className="permission-bar">
          <div className="permission-title"><IconShield size={14} /> {t.needApproval}{permission.title}</div>
          <div className="permission-actions">
            {permission.options.map((o: any) => (
              <button key={o.optionId} onClick={() => respondPermission(o.optionId)}>
                {humanPermLabel(o, t)}
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
