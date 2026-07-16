import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import ReactMarkdown from "react-markdown";
import { STRINGS, loadLang, saveLang, type Lang } from "./i18n";
import "./App.css";

type SessionEntry = {
  session_id: string;
  title: string;
  updated_at: string;
  num_messages: number;
  model_id?: string;
};

// ── 类型 ─────────────────────────────────────────────────────────

type ToolDiff = { path: string; oldText?: string; newText: string };

type ToolCallInfo = {
  toolCallId: string;
  title?: string;
  status?: string;
  kind?: string;
  diffs: ToolDiff[];
  output: string;
};

function extractToolContent(content: any[] | undefined): { diffs: ToolDiff[]; output: string } {
  const diffs: ToolDiff[] = [];
  let output = "";
  for (const c of content ?? []) {
    if (c?.type === "diff" && c.path) {
      diffs.push({ path: c.path, oldText: c.oldText ?? undefined, newText: c.newText ?? "" });
    } else if (c?.type === "content" && c.content?.type === "text") {
      output += c.content.text;
    }
  }
  return { diffs, output };
}

function DiffView({ diff }: { diff: ToolDiff }) {
  const oldLines = (diff.oldText ?? "").split("\n");
  const newLines = diff.newText.split("\n");
  return (
    <div className="diff">
      <div className="diff-path">{diff.path}</div>
      <pre>
        {diff.oldText !== undefined &&
          oldLines.map((l, i) => (
            <div key={"o" + i} className="line del">
              - {l}
            </div>
          ))}
        {newLines.map((l, i) => (
          <div key={"n" + i} className="line add">
            + {l}
          </div>
        ))}
      </pre>
    </div>
  );
}

type ChatItem =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string }
  | { kind: "thought"; text: string }
  | { kind: "tool"; call: ToolCallInfo };

type PermissionRequest = {
  id: number;
  title: string;
  options: { optionId: string; name: string; kind: string }[];
};

// ── 组件 ─────────────────────────────────────────────────────────

function App() {
  const [workspace, setWorkspace] = useState("D:\\WANCode\\scratch-agent-test");
  const [model, setModel] = useState("glm-5.2");
  const [models, setModels] = useState<string[]>([]);
  const [sessionId, setSessionId] = useState("");
  const [starting, setStarting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [items, setItems] = useState<ChatItem[]>([]);
  const [input, setInput] = useState("");
  const [permission, setPermission] = useState<PermissionRequest | null>(null);
  const [error, setError] = useState("");
  const [sessions, setSessions] = useState<SessionEntry[]>([]);
  const [mcpServers, setMcpServers] = useState<string[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [ctx, setCtx] = useState<{ used: number; total: number; pct: number } | null>(null);
  const [rewindPoints, setRewindPoints] = useState<any[] | null>(null);
  const [rewindMode, setRewindMode] = useState("all");
  const [mcpList, setMcpList] = useState<
    { name: string; command?: string; args: string[]; url?: string; enabled: boolean }[]
  >([]);
  const [mcpForm, setMcpForm] = useState({ name: "", command: "", args: "", url: "" });
  const [lang, setLang] = useState<Lang>(loadLang());
  const [version, setVersion] = useState("");
  const t = STRINGS[lang];

  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(""));
  }, []);

  async function refreshCtx() {
    try {
      const info = await invoke<any>("agent_session_info");
      const c = info?.result?.context ?? info?.result?.contextInfo ?? info?.context;
      if (c && typeof c.used === "number" && c.total) {
        setCtx({ used: c.used, total: c.total, pct: c.usagePct ?? Math.round((c.used / c.total) * 100) });
      }
    } catch {
      /* session gone */
    }
  }

  async function refreshMcpConfig() {
    try {
      setMcpList(await invoke<any[]>("mcp_config_list") as any);
    } catch {
      /* ignore */
    }
  }

  async function openRewind() {
    try {
      const r = await invoke<any>("agent_rewind_points");
      setRewindPoints(r?.rewindPoints ?? r?.rewind_points ?? []);
    } catch (e) {
      setError(String(e));
    }
  }

  async function doRewind(idx: number) {
    setRewindPoints(null);
    try {
      // Engine semantics: force=false is a pure dry-run preview;
      // force=true commits. Always preview first, then commit.
      const preview = await invoke<any>("agent_rewind", {
        targetPromptIndex: idx,
        mode: rewindMode,
        force: false,
      });
      if (preview?.conflicts?.length) {
        const ok = window.confirm(t.rewindConflicts(preview.conflicts.length));
        if (!ok) return;
      }
      const final = await invoke<any>("agent_rewind", {
        targetPromptIndex: idx,
        mode: rewindMode,
        force: true,
      });
      if (final?.success === false) {
        setError(t.rewindFailed + (final?.error ?? JSON.stringify(final)));
        return;
      }
      const reverted: string[] = final?.reverted_files ?? final?.revertedFiles ?? [];
      // Reload the session with replay so the UI reflects the truncated history.
      await startSession(sessionId);
      if (rewindMode !== "conversation_only") {
        setItems((prev) => [
          ...prev,
          {
            kind: "assistant",
            text: t.rewindDone(reverted.length, reverted.join("、")),
          },
        ]);
      }
    } catch (e) {
      setError(String(e));
    }
  }
  const bottomRef = useRef<HTMLDivElement>(null);
  const lastSentRef = useRef("");

  async function refreshSessions(ws: string) {
    try {
      setSessions(await invoke<SessionEntry[]>("agent_list_sessions", { workspace: ws }));
      setMcpServers(await invoke<string[]>("agent_list_mcp", { workspace: ws }));
    } catch {
      /* workspace may not exist yet */
    }
  }

  useEffect(() => {
    refreshSessions(workspace);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function pickFolder() {
    const dir = await openDialog({ directory: true, title: t.pickFolderTitle });
    if (typeof dir === "string" && dir) {
      setWorkspace(dir);
      refreshSessions(dir);
    }
  }

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [items, permission]);

  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];

    unsubs.push(
      listen<any>("agent://update", (e) => {
        const u = e.payload;
        if (!u || typeof u !== "object") return;
        if (u.sessionUpdate === "agent_message_chunk" && u.content?.type === "text") {
          appendStream("assistant", u.content.text);
        } else if (u.sessionUpdate === "user_message_chunk" && u.content?.type === "text") {
          // Skip the engine echo of the message we just sent locally;
          // replayed history (after resume) still renders.
          if (u.content.text === lastSentRef.current) {
            lastSentRef.current = "";
          } else {
            appendStream("user", u.content.text);
          }
        } else if (u.sessionUpdate === "agent_thought_chunk" && u.content?.type === "text") {
          appendStream("thought", u.content.text);
        } else if (u.sessionUpdate === "tool_call") {
          const { diffs, output } = extractToolContent(u.content);
          setItems((prev) => [
            ...prev,
            {
              kind: "tool",
              call: {
                toolCallId: u.toolCallId,
                title: u.title,
                status: u.status,
                kind: u.kind,
                diffs,
                output,
              },
            },
          ]);
        } else if (u.sessionUpdate === "tool_call_update") {
          const { diffs, output } = extractToolContent(u.content);
          setItems((prev) =>
            prev.map((it) =>
              it.kind === "tool" && it.call.toolCallId === u.toolCallId
                ? {
                    ...it,
                    call: {
                      ...it.call,
                      status: u.status ?? it.call.status,
                      title: u.title ?? it.call.title,
                      diffs: diffs.length ? diffs : it.call.diffs,
                      output: output || it.call.output,
                    },
                  }
                : it,
            ),
          );
        }
      }),
    );

    unsubs.push(
      listen<any>("agent://permission", (e) => {
        const p = e.payload;
        setPermission({
          id: p.id,
          title: p.request?.toolCall?.title ?? p.request?.toolCall?.kind ?? "工具调用请求",
          options: (p.request?.options ?? []).map((o: any) => ({
            optionId: o.optionId,
            name: o.name,
            kind: o.kind,
          })),
        });
      }),
    );

    unsubs.push(
      listen<any>("agent://turn-end", (e) => {
        setBusy(false);
        if (e.payload && e.payload.ok === false) {
          setError(String(e.payload.error ?? "未知错误"));
        }
        refreshCtx();
      }),
    );

    return () => {
      unsubs.forEach((p) => p.then((un) => un()));
    };
  }, []);

  function appendStream(kind: "assistant" | "thought" | "user", text: string) {
    setItems((prev) => {
      const last = prev[prev.length - 1];
      if (last && last.kind === kind) {
        const copy = prev.slice(0, -1);
        return [...copy, { kind, text: (last as any).text + text } as ChatItem];
      }
      return [...prev, { kind, text } as ChatItem];
    });
  }

  async function startSession(resume?: string) {
    setStarting(true);
    setError("");
    setItems([]);
    setSessionId("");
    try {
      const r = await invoke<{ session_id: string; models: string[] }>("agent_start", {
        workspace,
        model,
        resume: resume ?? null,
      });
      setSessionId(r.session_id);
      if (r.models?.length) setModels(r.models);
      refreshSessions(workspace);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  }

  async function send() {
    const text = input.trim();
    if (!text || busy || !sessionId) return;
    setInput("");
    setError("");
    lastSentRef.current = text;
    setItems((prev) => [...prev, { kind: "user", text }]);
    setBusy(true);
    invoke("agent_prompt", { text }).catch((e) => {
      setError(String(e));
      setBusy(false);
    });
  }

  async function respondPermission(optionId: string | null) {
    if (!permission) return;
    const id = permission.id;
    setPermission(null);
    await invoke("agent_permission_respond", { id, optionId }).catch((e) =>
      setError(String(e)),
    );
  }

  const examples = t.examples;

  return (
    <main className="chat-app">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark">W</div>
          <div className="brand-name">WanCode</div>
        </div>
        <input
          value={workspace}
          onChange={(e) => setWorkspace(e.currentTarget.value)}
          placeholder={t.workspacePlaceholder}
          title={t.workspacePlaceholder}
          style={{ flex: 1 }}
          disabled={!!sessionId}
        />
        <button className="ghost" onClick={pickFolder} disabled={!!sessionId} title={t.browseFolder}>
          📁
        </button>
        <select value={model} onChange={(e) => setModel(e.currentTarget.value)} disabled={!!sessionId}>
          {(models.length ? models : ["glm-5.2", "glm-5-turbo", "glm-4-flash", "deepseek-chat", "deepseek-reasoner"]).map(
            (m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ),
          )}
        </select>
        {sessionId ? (
          <span className="connected-pill">
            <span className="dot" />
            {t.connected}
          </span>
        ) : (
          <button onClick={() => startSession()} disabled={starting}>
            {starting ? t.starting : t.openWorkspace}
          </button>
        )}
        {sessionId && (
          <button className="ghost" title={t.rewindTooltip} onClick={openRewind}>
            ⏪
          </button>
        )}
        <button
          className="ghost"
          title={t.settings}
          onClick={() => {
            refreshMcpConfig();
            setShowSettings(true);
          }}
        >
          ⚙
        </button>
      </header>

      {ctx && sessionId && (
        <div className="ctx-bar" title={`${ctx.used.toLocaleString()} / ${ctx.total.toLocaleString()} tokens`}>
          <div
            className={`ctx-fill ${ctx.pct > 80 ? "hot" : ""}`}
            style={{ width: `${Math.min(100, ctx.pct)}%` }}
          />
          <span className="ctx-label">
            {t.ctxLabel(ctx.pct, Math.round(ctx.used / 1000), Math.round(ctx.total / 1000))}
          </span>
        </div>
      )}

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

      {showSettings && (
        <div className="modal-mask" onClick={() => setShowSettings(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.settingsTitle}</div>
            <div className="modal-section">
              <div className="modal-label">{t.language}</div>
              <select
                value={lang}
                onChange={(e) => {
                  const l = e.currentTarget.value as Lang;
                  setLang(l);
                  saveLang(l);
                }}
              >
                <option value="zh">中文</option>
                <option value="en">English</option>
              </select>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.availableModels(models.length || 5)}</div>
              <div className="modal-body">
                {(models.length ? models : ["glm-5.2", "glm-5-turbo", "glm-4-flash", "deepseek-chat", "deepseek-reasoner"]).join("、")}
              </div>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.mcpSection}</div>
              <div className="mcp-list">
                {mcpList.length === 0 && <div className="sidebar-empty">{t.notConfigured}</div>}
                {mcpList.map((s) => (
                  <div key={s.name} className="mcp-item">
                    <div className="mcp-info">
                      <b>{s.name}</b>
                      <span className="mcp-detail">
                        {s.command ? `${s.command} ${s.args.join(" ")}` : s.url}
                      </span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.mcpDelete}
                      onClick={async () => {
                        await invoke("mcp_config_remove", { name: s.name }).catch((e) => setError(String(e)));
                        refreshMcpConfig();
                        refreshSessions(workspace);
                      }}
                    >
                      ✕
                    </button>
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.mcpName}
                  value={mcpForm.name}
                  onChange={(e) => setMcpForm({ ...mcpForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpCommand}
                  value={mcpForm.command}
                  onChange={(e) => setMcpForm({ ...mcpForm, command: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpArgs}
                  value={mcpForm.args}
                  onChange={(e) => setMcpForm({ ...mcpForm, args: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpUrl}
                  value={mcpForm.url}
                  onChange={(e) => setMcpForm({ ...mcpForm, url: e.currentTarget.value })}
                />
                <button
                  onClick={async () => {
                    try {
                      await invoke("mcp_config_upsert", {
                        name: mcpForm.name,
                        command: mcpForm.command || null,
                        args: mcpForm.args.trim() ? mcpForm.args.trim().split(/\s+/) : [],
                        url: mcpForm.url || null,
                      });
                      setMcpForm({ name: "", command: "", args: "", url: "" });
                      refreshMcpConfig();
                      refreshSessions(workspace);
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t.mcpAdd}
                </button>
              </div>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.projectMemory}</div>
              <div className="modal-body">{t.projectMemoryHelp}</div>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.configFile}</div>
              <div className="modal-body mono">{t.configHelp}</div>
            </div>
            <div className="modal-footer">
              <span className="version-tag">
                WanCode {version ? `v${version}` : ""}
                <a
                  className="update-link"
                  onClick={() =>
                    openUrl("https://github.com/ThomasWan123/grok-build").catch(() => {})
                  }
                >
                  {t.checkUpdate}
                </a>
              </span>
              <button onClick={() => setShowSettings(false)}>{t.close}</button>
            </div>
          </div>
        </div>
      )}

      <div className="body-row">
        <aside className="sidebar">
          <div className="sidebar-head">
            <span>{t.sessions}</span>
            <button
              className="ghost small"
              title={t.newSession}
              onClick={() => {
                setSessionId("");
                setItems([]);
              }}
            >
              ＋
            </button>
          </div>
          <div className="session-list">
            {sessions.length === 0 && <div className="sidebar-empty">{t.noSessions}</div>}
            {sessions.map((s) => (
              <div
                key={s.session_id}
                className={`session-item ${s.session_id === sessionId ? "active" : ""}`}
                onClick={() => !starting && startSession(s.session_id)}
                title={s.session_id}
              >
                <div className="session-title">{s.title}</div>
                <div className="session-meta">
                  {s.updated_at.slice(0, 16).replace("T", " ")} · {s.num_messages} {t.messagesUnit}
                  {s.model_id ? ` · ${s.model_id}` : ""}
                </div>
              </div>
            ))}
          </div>
          <div className="sidebar-foot">
            {t.mcpFooter}
            {mcpServers.length ? mcpServers.join("、") : t.notConfigured}
          </div>
        </aside>

        <div className="main-col">
      {items.length === 0 && !busy && (
        <div className="empty-state">
          <div className="empty-logo">W</div>
          <div className="empty-title">{t.appTagline}</div>
          <div className="empty-sub">{sessionId ? t.emptySubReady : t.emptySubStart}</div>
          <div className="chips">
            {examples.map((ex) => (
              <div
                key={ex}
                className="chip"
                onClick={() => sessionId && setInput(ex)}
              >
                {ex}
              </div>
            ))}
          </div>
          <div className="empty-hint">{t.emptyHint}</div>
        </div>
      )}

      <section className="messages" style={items.length === 0 && !busy ? { display: "none" } : undefined}>
        {items.map((it, i) => {
          if (it.kind === "user")
            return (
              <div key={i} className="msg user">
                {it.text}
              </div>
            );
          if (it.kind === "assistant")
            return (
              <div key={i} className="msg assistant">
                <ReactMarkdown>{it.text}</ReactMarkdown>
              </div>
            );
          if (it.kind === "thought")
            return (
              <details key={i} className="msg thought">
                <summary>{t.thinking}</summary>
                <ReactMarkdown>{it.text}</ReactMarkdown>
              </details>
            );
          return (
            <div key={i} className="msg tool">
              🔧 {it.call.title ?? it.call.kind ?? t.toolCall}
              <span className={`status ${it.call.status ?? ""}`}> {it.call.status ?? ""}</span>
              {it.call.diffs.map((d, j) => (
                <DiffView key={j} diff={d} />
              ))}
              {it.call.output && (
                <details className="tool-output">
                  <summary>{t.output}</summary>
                  <pre>{it.call.output}</pre>
                </details>
              )}
            </div>
          );
        })}
        {busy && <div className="msg pending">{t.thinkingNow}</div>}
        {error && <div className="msg error">⚠ {error}</div>}
        <div ref={bottomRef} />
      </section>

      {permission && (
        <div className="permission-bar">
          <div className="permission-title">🔐 {t.needApproval}{permission.title}</div>
          <div className="permission-actions">
            {permission.options.map((o) => (
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

      <footer className="composer">
        <textarea
          value={input}
          onChange={(e) => setInput(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
          placeholder={sessionId ? t.composerPlaceholder : t.composerLocked}
          disabled={!sessionId}
          rows={3}
        />
        {busy ? (
          <button
            className="send stop"
            onClick={() => invoke("agent_cancel").catch(() => {})}
            title={t.stopTitle}
          >
            {t.stop}
          </button>
        ) : (
          <button className="send" onClick={send} disabled={!sessionId || !input.trim()}>
            {t.send}
          </button>
        )}
      </footer>
        </div>
      </div>
    </main>
  );
}

export default App;
