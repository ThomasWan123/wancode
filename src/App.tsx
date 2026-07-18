import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
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

type TreeNode = { name: string; path: string; children?: Record<string, TreeNode> };

function buildTree(paths: string[]): TreeNode {
  const root: TreeNode = { name: "", path: "", children: {} };
  for (const p of paths) {
    const parts = p.split("/");
    let node = root;
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      acc = acc ? `${acc}/${parts[i]}` : parts[i];
      node.children ??= {};
      node.children[parts[i]] ??= { name: parts[i], path: acc, children: i < parts.length - 1 ? {} : undefined };
      node = node.children[parts[i]];
    }
  }
  return root;
}

function TreeView({ node, onPick, depth = 0 }: { node: TreeNode; onPick: (p: string) => void; depth?: number }) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const entries = Object.values(node.children ?? {}).sort((a, b) => {
    const ad = !!a.children, bd = !!b.children;
    if (ad !== bd) return ad ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  return (
    <>
      {entries.map((c) => {
        const isDir = !!c.children;
        return (
          <div key={c.path}>
            <div
              className="tree-row"
              style={{ paddingLeft: 8 + depth * 12 }}
              onClick={() => (isDir ? setOpen((o) => ({ ...o, [c.path]: !o[c.path] })) : onPick(c.path))}
            >
              <span className="tree-icon">{isDir ? (open[c.path] ? "📂" : "📁") : "📄"}</span>
              <span className="tree-name">{c.name}</span>
            </div>
            {isDir && open[c.path] && <TreeView node={c} onPick={onPick} depth={depth + 1} />}
          </div>
        );
      })}
    </>
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
  toolCallId?: string;
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
  const [settingsTab, setSettingsTab] = useState<"general" | "models" | "mcp" | "skills" | "hooks" | "about">("models");
  const [ctx, setCtx] = useState<{ used: number; total: number; pct: number } | null>(null);
  const [rewindPoints, setRewindPoints] = useState<any[] | null>(null);
  const [rewindMode, setRewindMode] = useState("all");
  const [mcpList, setMcpList] = useState<
    { name: string; command?: string; args: string[]; url?: string; enabled: boolean }[]
  >([]);
  const [mcpForm, setMcpForm] = useState({ name: "", command: "", args: "", url: "" });
  const [hooks, setHooks] = useState<{ event: string; command: string }[]>([]);
  const [hookForm, setHookForm] = useState({ event: "PostToolUse", command: "" });
  const [modelList, setModelList] = useState<any[]>([]);
  const [modelForm, setModelForm] = useState({ key: "", name: "", model: "", base_url: "", api_key: "" });
  const [modelTestMsg, setModelTestMsg] = useState("");

  const MODEL_PRESETS: Record<string, { name: string; model: string; base_url: string }> = {
    DeepSeek: { name: "DeepSeek V3", model: "deepseek-chat", base_url: "https://api.deepseek.com/v1" },
    "智谱 GLM": { name: "智谱 GLM-4-Flash", model: "glm-4-flash", base_url: "https://open.bigmodel.cn/api/paas/v4" },
    OpenAI: { name: "GPT-4o", model: "gpt-4o", base_url: "https://api.openai.com/v1" },
    Ollama: { name: "Ollama (本地)", model: "qwen2.5-coder", base_url: "http://localhost:11434/v1" },
  };

  async function refreshModels() {
    try {
      setModelList(await invoke<any[]>("model_list"));
    } catch {
      /* ignore */
    }
  }

  async function testModel() {
    setModelTestMsg(t.modelTesting);
    try {
      const reply = await invoke<string>("model_test", {
        baseUrl: modelForm.base_url,
        model: modelForm.model,
        apiKey: modelForm.api_key || null,
        key: modelForm.key || null,
      });
      setModelTestMsg(t.modelTestOk(reply));
    } catch (e) {
      setModelTestMsg(t.modelTestFail(String(e)));
    }
  }

  async function saveModel() {
    if (!modelForm.key.trim() || !modelForm.model.trim() || !modelForm.base_url.trim()) return;
    try {
      await invoke("model_upsert", {
        key: modelForm.key,
        name: modelForm.name || modelForm.key,
        model: modelForm.model,
        baseUrl: modelForm.base_url,
        apiKey: modelForm.api_key || null,
      });
      setModelForm({ key: "", name: "", model: "", base_url: "", api_key: "" });
      setModelTestMsg("");
      refreshModels();
    } catch (e) {
      setError(String(e));
    }
  }
  const [lang, setLang] = useState<Lang>(loadLang());
  const [theme, setTheme] = useState<"dark" | "light">(
    (localStorage.getItem("wancode-theme") as "dark" | "light") || "dark",
  );
  const [version, setVersion] = useState("");

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("wancode-theme", theme);
  }, [theme]);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<SessionEntry[] | null>(null);
  const [updateMsg, setUpdateMsg] = useState("");
  const [fileList, setFileList] = useState<string[]>([]);
  const [popup, setPopup] = useState<{ kind: "at" | "slash"; query: string; sel: number } | null>(null);
  const [terminalLines, setTerminalLines] = useState<string[]>([]);
  const [showTerminal, setShowTerminal] = useState(false);
  const [sidebarTab, setSidebarTab] = useState<"sessions" | "files">("sessions");
  const [planSteps, setPlanSteps] = useState<{ content: string; status?: string }[]>([]);
  const [gitInfo, setGitInfo] = useState<any>(null);
  const [showGit, setShowGit] = useState(false);
  const [pastedImages, setPastedImages] = useState<{ data: string; mime: string; preview: string }[]>([]);
  const [planMode, setPlanMode] = useState(false);
  const [planApproval, setPlanApproval] = useState<{ id: number; planContent: string } | null>(null);
  const [planFeedback, setPlanFeedback] = useState("");
  const [skills, setSkills] = useState<{ name: string; description: string; path: string }[]>([]);
  const [skillForm, setSkillForm] = useState({ name: "", description: "" });

  async function respondPlan(outcome: string) {
    if (!planApproval) return;
    const id = planApproval.id;
    const fb = planFeedback.trim() || null;
    setPlanApproval(null);
    setPlanFeedback("");
    await invoke("agent_plan_respond", { id, outcome, feedback: fb }).catch((e) => setError(String(e)));
  }

  async function togglePlanMode() {
    const next = !planMode;
    setPlanMode(next);
    await invoke("agent_set_mode", { mode: next ? "plan" : "default" }).catch((e) => {
      setError(String(e));
      setPlanMode(!next);
    });
  }

  async function refreshGit() {
    try {
      setGitInfo(await invoke<any>("git_status", { workspace }));
    } catch {
      setGitInfo(null);
    }
  }
  const taRef = useRef<HTMLTextAreaElement>(null);
  const t = STRINGS[lang];

  const SLASH_COMMANDS = [
    { cmd: "/commit", desc: lang === "zh" ? "让 AI 提交当前改动" : "Ask AI to commit changes", prompt: "Review the git diff, then create a well-formed git commit for the current changes." },
    { cmd: "/review", desc: lang === "zh" ? "审查未提交的改动" : "Review uncommitted changes", prompt: "Review my uncommitted changes for bugs, and summarize what changed." },
    { cmd: "/test", desc: lang === "zh" ? "运行测试" : "Run the test suite", prompt: "Detect and run this project's test suite, then report the results." },
    { cmd: "/explain", desc: lang === "zh" ? "解释这个项目" : "Explain this project", prompt: "Explain this project's structure, entry points, and how the pieces fit together." },
    { cmd: "/compact", desc: lang === "zh" ? "压缩上下文" : "Compact the context", action: "compact" as const },
    { cmd: "/rewind", desc: lang === "zh" ? "打开回滚" : "Open rewind", action: "rewind" as const },
    { cmd: "/clear", desc: lang === "zh" ? "开新会话" : "Start a new session", action: "clear" as const },
  ];

  async function runSearch(q: string) {
    setSearchQuery(q);
    if (!q.trim()) {
      setSearchHits(null);
      return;
    }
    try {
      if (!sessionId) await startSession();
      const r = await invoke<any>("agent_session_search", { query: q.trim(), workspace });
      const hits = r?.result?.results ?? r?.results ?? [];
      setSearchHits(
        hits.map((h: any) => ({
          session_id: h.sessionId ?? h.session_id,
          title: h.summary || h.snippet || "(untitled)",
          updated_at: h.updatedAt ?? h.updated_at ?? "",
          num_messages: 0,
          model_id: undefined,
        })),
      );
    } catch (e) {
      setError(String(e));
      setSearchHits([]);
    }
  }

  async function runUpdate() {
    setUpdateMsg(t.checkingUpdate);
    try {
      const update = await checkUpdate();
      if (!update) {
        setUpdateMsg(t.upToDate(version));
        return;
      }
      setUpdateMsg(t.updateAvailable(update.version));
      await update.downloadAndInstall();
      setUpdateMsg(t.updateInstalling);
      await relaunch();
    } catch (e) {
      setUpdateMsg(t.updateFailed + String(e));
    }
  }

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
    try {
      setHooks(await invoke<{ event: string; command: string }[]>("hooks_list"));
    } catch {
      /* ignore */
    }
    try {
      setSkills(await invoke<{ name: string; description: string; path: string }[]>("skills_list"));
    } catch {
      /* ignore */
    }
    refreshModels();
  }

  async function saveHooks(next: { event: string; command: string }[]) {
    setHooks(next);
    await invoke("hooks_save", { entries: next }).catch((e) => setError(String(e)));
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
  const execIdsRef = useRef<Set<string>>(new Set());

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
        if (u.sessionUpdate === "current_mode_update" || u.sessionUpdate === "current_mode") {
          const m = u.currentModeId ?? u.current_mode_id;
          if (m) setPlanMode(m === "plan");
        } else if (u.sessionUpdate === "plan") {
          const entries = u.entries ?? u.plan?.entries ?? [];
          setPlanSteps(
            entries.map((e: any) => ({
              content: e.content ?? e.title ?? String(e),
              status: e.status,
            })),
          );
        } else if (u.sessionUpdate === "agent_message_chunk" && u.content?.type === "text") {
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
          const isExec =
            u.kind === "execute" || /^(execute|run)\b/i.test(u.title ?? "");
          if (isExec) {
            if (u.toolCallId) execIdsRef.current.add(u.toolCallId);
            const cmd = u.title ?? "command";
            setTerminalLines((prev) => [...prev, `$ ${cmd}`, ...(output ? [output] : [])]);
            setShowTerminal(true);
          }
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
          if (output && u.toolCallId && execIdsRef.current.has(u.toolCallId)) {
            setTerminalLines((prev) =>
              prev[prev.length - 1] === output ? prev : [...prev, output],
            );
            setShowTerminal(true);
          }
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
          toolCallId: p.request?.toolCall?.toolCallId,
          options: (p.request?.options ?? []).map((o: any) => ({
            optionId: o.optionId,
            name: o.name,
            kind: o.kind,
          })),
        });
      }),
    );

    unsubs.push(
      listen<any>("agent://plan-approval", (e) => {
        setPlanApproval({ id: e.payload.id, planContent: e.payload.planContent ?? "" });
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
      invoke<string[]>("list_workspace_files", { workspace })
        .then(setFileList)
        .catch(() => {});
      refreshGit();
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  }

  function sendText(text: string, imgs: { data: string; mime: string; preview: string }[] = []) {
    const t = text.trim();
    if ((!t && imgs.length === 0) || busy || !sessionId) return;
    setError("");
    lastSentRef.current = t;
    setPlanSteps([]);
    const label = imgs.length ? `${t}${t ? "  " : ""}🖼️×${imgs.length}` : t;
    setItems((prev) => [...prev, { kind: "user", text: label }]);
    setBusy(true);
    invoke("agent_prompt", {
      text: t,
      images: imgs.length ? imgs.map((i) => ({ data: i.data, mime: i.mime })) : null,
    }).catch((e) => {
      setError(String(e));
      setBusy(false);
    });
  }

  function send() {
    if (popup) return;
    const text = input.trim();
    if (!text && pastedImages.length === 0) return;
    const imgs = pastedImages;
    setInput("");
    setPastedImages([]);
    sendText(text, imgs);
  }

  function onPaste(e: React.ClipboardEvent<HTMLTextAreaElement>) {
    const items = Array.from(e.clipboardData.items).filter((i) => i.type.startsWith("image/"));
    if (!items.length) return;
    e.preventDefault();
    for (const it of items) {
      const file = it.getAsFile();
      if (!file) continue;
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = String(reader.result);
        const base64 = dataUrl.split(",")[1] ?? "";
        setPastedImages((prev) => [...prev, { data: base64, mime: file.type, preview: dataUrl }]);
      };
      reader.readAsDataURL(file);
    }
  }

  // ── @ file-mention / slash-command popup ──────────────────────────
  const popupItems: { label: string; desc?: string }[] =
    popup?.kind === "at"
      ? fileList
          .filter((f) => f.toLowerCase().includes(popup.query.toLowerCase()))
          .slice(0, 8)
          .map((f) => ({ label: f }))
      : popup?.kind === "slash"
        ? SLASH_COMMANDS.filter((c) => c.cmd.startsWith("/" + popup.query.replace(/^\//, ""))).map(
            (c) => ({ label: c.cmd, desc: c.desc }),
          )
        : [];

  function onComposerChange(v: string) {
    setInput(v);
    const caret = taRef.current?.selectionStart ?? v.length;
    const before = v.slice(0, caret);
    // slash: only when it's the very first char of the input
    if (/^\/[\w-]*$/.test(before) && v === before) {
      setPopup({ kind: "slash", query: before, sel: 0 });
      return;
    }
    // @: token after a whitespace/start, no spaces inside
    const m = before.match(/(?:^|\s)@([^\s@]*)$/);
    if (m) {
      setPopup({ kind: "at", query: m[1], sel: 0 });
    } else {
      setPopup(null);
    }
  }

  function acceptPopup(index: number) {
    if (!popup) return;
    if (popup.kind === "slash") {
      const c = SLASH_COMMANDS.filter((c) =>
        c.cmd.startsWith("/" + popup.query.replace(/^\//, "")),
      )[index];
      if (!c) return;
      setPopup(null);
      setInput("");
      if ("action" in c && c.action) {
        if (c.action === "clear") {
          setSessionId("");
          setItems([]);
        } else if (c.action === "rewind") {
          openRewind();
        } else if (c.action === "compact") {
          setInput("");
          sendText("Please compact our conversation, preserving key decisions and current task state.");
        }
      } else if ("prompt" in c && c.prompt) {
        sendText(c.prompt);
      }
      return;
    }
    // at
    const files = fileList
      .filter((f) => f.toLowerCase().includes(popup.query.toLowerCase()))
      .slice(0, 8);
    const picked = files[index];
    if (!picked) return;
    const caret = taRef.current?.selectionStart ?? input.length;
    const before = input.slice(0, caret).replace(/@([^\s@]*)$/, "@" + picked + " ");
    const after = input.slice(caret);
    setInput(before + after);
    setPopup(null);
    taRef.current?.focus();
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
          <button
            className={`ghost plan-toggle ${planMode ? "on" : ""}`}
            title={planMode ? t.planModeOn : t.planModeOff}
            onClick={togglePlanMode}
          >
            📋 {t.planMode}
          </button>
        )}
        {sessionId && (
          <button className="ghost" title={t.rewindTooltip} onClick={openRewind}>
            ⏪
          </button>
        )}
        {sessionId && (
          <button
            className="ghost"
            title={t.git}
            onClick={() => {
              refreshGit();
              setShowGit(true);
            }}
          >
            ⑂
          </button>
        )}
        <button
          className="ghost"
          title={t.toggleTheme}
          onClick={() => setTheme((th) => (th === "dark" ? "light" : "dark"))}
        >
          {theme === "dark" ? "☀" : "🌙"}
        </button>
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
          <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
            <nav className="settings-nav">
              <div className="settings-nav-title">{t.settingsTitle}</div>
              {([
                ["general", t.navGeneral],
                ["models", t.navModels],
                ["mcp", t.navMcp],
                ["skills", t.navSkills],
                ["hooks", t.navHooks],
                ["about", t.navAbout],
              ] as const).map(([id, label]) => (
                <button
                  key={id}
                  className={`settings-nav-item ${settingsTab === id ? "active" : ""}`}
                  onClick={() => setSettingsTab(id)}
                >
                  {label}
                </button>
              ))}
            </nav>
            <div className="settings-content">
            {settingsTab === "general" && (
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
              <div className="modal-label" style={{ marginTop: 16 }}>{t.themeLabel}</div>
              <select value={theme} onChange={(e) => setTheme(e.currentTarget.value as "dark" | "light")}>
                <option value="dark">{t.themeDark}</option>
                <option value="light">{t.themeLight}</option>
              </select>
            </div>
            )}
            {settingsTab === "models" && (
            <div className="modal-section">
              <div className="modal-label">{t.modelsSection}</div>
              <div className="mcp-list">
                {modelList.length === 0 && <div className="sidebar-empty">{t.modelsEmpty}</div>}
                {modelList.map((m) => (
                  <div key={m.key} className="mcp-item">
                    <div className="mcp-info">
                      <b>
                        {m.name}{" "}
                        <span className={m.has_key ? "key-ok" : "key-warn"}>
                          {m.has_key ? t.modelKeyStored : t.modelKeyMissing}
                        </span>
                      </b>
                      <span className="mcp-detail">
                        {m.model} · {m.base_url}
                      </span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.modelDelete}
                      onClick={async () => {
                        await invoke("model_remove", { key: m.key }).catch((e) => setError(String(e)));
                        refreshModels();
                      }}
                    >
                      ✕
                    </button>
                  </div>
                ))}
              </div>
              <div className="model-presets">
                <span className="preset-label">{t.modelPreset}:</span>
                {Object.entries(MODEL_PRESETS).map(([label, p]) => (
                  <button
                    key={label}
                    className="chip preset-chip"
                    onClick={() =>
                      setModelForm({
                        key: modelForm.key || label.toLowerCase().replace(/\s+/g, "-"),
                        name: p.name,
                        model: p.model,
                        base_url: p.base_url,
                        api_key: modelForm.api_key,
                      })
                    }
                  >
                    {label}
                  </button>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.modelKeyField}
                  value={modelForm.key}
                  onChange={(e) => setModelForm({ ...modelForm, key: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelDisplayName}
                  value={modelForm.name}
                  onChange={(e) => setModelForm({ ...modelForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelIdField}
                  value={modelForm.model}
                  onChange={(e) => setModelForm({ ...modelForm, model: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelBaseUrl}
                  value={modelForm.base_url}
                  onChange={(e) => setModelForm({ ...modelForm, base_url: e.currentTarget.value })}
                />
                <input
                  type="password"
                  placeholder={t.modelApiKey}
                  value={modelForm.api_key}
                  onChange={(e) => setModelForm({ ...modelForm, api_key: e.currentTarget.value })}
                />
                <div style={{ display: "flex", gap: 6 }}>
                  <button style={{ flex: 1 }} onClick={saveModel}>
                    {t.modelSave}
                  </button>
                  <button
                    className="ghost"
                    onClick={testModel}
                    disabled={!modelForm.base_url || !modelForm.model}
                  >
                    {t.modelTest}
                  </button>
                </div>
                {modelTestMsg && <div className="model-test-msg">{modelTestMsg}</div>}
              </div>
              <div className="modal-hint">{t.modelsHint}</div>
            </div>
            )}
            {settingsTab === "mcp" && (
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
            )}
            {settingsTab === "skills" && (
            <div className="modal-section">
              <div className="modal-label">{t.skillsSection}</div>
              <div className="mcp-list">
                {skills.length === 0 && <div className="sidebar-empty">{t.skillsEmpty}</div>}
                {skills.map((sk) => (
                  <div key={sk.name} className="mcp-item">
                    <div className="mcp-info">
                      <b>{sk.name}</b>
                      <span className="mcp-detail">{sk.description}</span>
                    </div>
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.skillsName}
                  value={skillForm.name}
                  onChange={(e) => setSkillForm({ ...skillForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.skillsDesc}
                  value={skillForm.description}
                  onChange={(e) => setSkillForm({ ...skillForm, description: e.currentTarget.value })}
                />
                <div style={{ display: "flex", gap: 6 }}>
                  <button
                    style={{ flex: 1 }}
                    onClick={async () => {
                      if (!skillForm.name.trim()) return;
                      try {
                        await invoke("skills_create", {
                          name: skillForm.name.trim(),
                          description: skillForm.description,
                        });
                        setSkillForm({ name: "", description: "" });
                        setSkills(await invoke("skills_list"));
                      } catch (e) {
                        setError(String(e));
                      }
                    }}
                  >
                    {t.skillsCreate}
                  </button>
                  <button className="ghost" onClick={() => invoke("skills_open").catch((e) => setError(String(e)))}>
                    {t.skillsOpen}
                  </button>
                </div>
              </div>
              <div className="modal-hint">{t.skillsHint}</div>
            </div>
            )}
            {settingsTab === "hooks" && (
            <div className="modal-section">
              <div className="modal-label">{t.hooksSection}</div>
              <div className="mcp-list">
                {hooks.length === 0 && <div className="sidebar-empty">{t.hooksEmpty}</div>}
                {hooks.map((h, i) => (
                  <div key={i} className="mcp-item">
                    <div className="mcp-info">
                      <b>{h.event}</b>
                      <span className="mcp-detail">{h.command}</span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.mcpDelete}
                      onClick={() => saveHooks(hooks.filter((_, j) => j !== i))}
                    >
                      ✕
                    </button>
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <select
                  value={hookForm.event}
                  onChange={(e) => setHookForm({ ...hookForm, event: e.currentTarget.value })}
                >
                  {["PreToolUse", "PostToolUse", "SessionStart", "Stop", "UserPromptSubmit"].map((ev) => (
                    <option key={ev} value={ev}>
                      {ev}
                    </option>
                  ))}
                </select>
                <input
                  placeholder={t.hooksCommand}
                  value={hookForm.command}
                  onChange={(e) => setHookForm({ ...hookForm, command: e.currentTarget.value })}
                />
                <button
                  onClick={() => {
                    if (!hookForm.command.trim()) return;
                    saveHooks([...hooks, { event: hookForm.event, command: hookForm.command.trim() }]);
                    setHookForm({ event: hookForm.event, command: "" });
                  }}
                >
                  {t.hooksAdd}
                </button>
              </div>
              <div className="modal-hint">{t.hooksHint}</div>
            </div>
            )}
            {settingsTab === "about" && (
            <>
            <div className="modal-section">
              <div className="modal-label">{t.projectMemory}</div>
              <div className="modal-body">{t.projectMemoryHelp}</div>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.configFile}</div>
              <div className="modal-body mono">{t.configHelp}</div>
            </div>
            <div className="modal-section">
              <div className="modal-label">WanCode {version ? `v${version}` : ""}</div>
              <div className="about-actions">
                <button className="ghost" onClick={runUpdate}>{t.checkUpdate}</button>
                {updateMsg && <span className="update-msg">{updateMsg}</span>}
              </div>
            </div>
            </>
            )}
            <div className="settings-footer">
              <button onClick={() => setShowSettings(false)}>{t.close}</button>
            </div>
            </div>
          </div>
        </div>
      )}

      {showGit && (
        <div className="modal-mask" onClick={() => setShowGit(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">⑂ {t.git}</div>
            {gitInfo?.isRepo === false || !gitInfo ? (
              <div className="modal-body">{t.gitNotRepo}</div>
            ) : (
              <>
                <div className="modal-section">
                  <div className="modal-label">{gitInfo.branch}</div>
                  <div className="git-files">
                    {(!gitInfo.files || gitInfo.files.length === 0) && (
                      <div className="sidebar-empty">{t.gitClean}</div>
                    )}
                    {(gitInfo.files ?? []).map((f: any) => (
                      <div key={f.path} className="git-file">
                        <span className="git-xy">{f.xy || "??"}</span>
                        <span className="git-path">{f.path}</span>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="git-actions">
                  <button
                    disabled={!gitInfo.files?.length}
                    onClick={() => {
                      setShowGit(false);
                      sendText(
                        "Review the git diff of my uncommitted changes, then create a well-formed git commit with a clear message.",
                      );
                    }}
                  >
                    {t.gitCommit}
                  </button>
                  <button
                    className="ghost"
                    disabled={!gitInfo.files?.length}
                    onClick={() => {
                      setShowGit(false);
                      sendText("Review my uncommitted changes for bugs and summarize what changed.");
                    }}
                  >
                    {t.gitReview}
                  </button>
                </div>
              </>
            )}
            <div className="modal-footer">
              <span />
              <button onClick={() => setShowGit(false)}>{t.close}</button>
            </div>
          </div>
        </div>
      )}

      {planApproval && (
        <div className="modal-mask">
          <div className="modal plan-approval-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">{t.planApprovalTitle}</div>
            <div className="plan-approval-body">
              <ReactMarkdown>{planApproval.planContent || "_(empty plan)_"}</ReactMarkdown>
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

      <div className="body-row">
        <aside className="sidebar">
          <div className="sidebar-tabs">
            <button
              className={`tab ${sidebarTab === "sessions" ? "active" : ""}`}
              onClick={() => setSidebarTab("sessions")}
            >
              {t.tabSessions}
            </button>
            <button
              className={`tab ${sidebarTab === "files" ? "active" : ""}`}
              onClick={() => setSidebarTab("files")}
            >
              {t.tabFiles}
            </button>
            {sidebarTab === "sessions" && (
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
            )}
          </div>

          {sidebarTab === "files" ? (
            <div className="session-list tree-list">
              {fileList.length === 0 ? (
                <div className="sidebar-empty">{sessionId ? "—" : t.emptySubStart}</div>
              ) : (
                <TreeView node={buildTree(fileList)} onPick={(p) => setInput((v) => v + (v && !v.endsWith(" ") ? " " : "") + "@" + p + " ")} />
              )}
            </div>
          ) : (
          <>
          <input
            className="session-search"
            value={searchQuery}
            placeholder={t.searchPlaceholder}
            onChange={(e) => runSearch(e.currentTarget.value)}
          />
          <div className="session-list">
            {searchHits !== null && searchHits.length === 0 && (
              <div className="sidebar-empty">{t.searchNoResults}</div>
            )}
            {searchHits === null && sessions.length === 0 && (
              <div className="sidebar-empty">{t.noSessions}</div>
            )}
            {(searchHits ?? sessions).map((s) => (
              <div
                key={s.session_id}
                className={`session-item ${s.session_id === sessionId ? "active" : ""}`}
                onClick={() => !starting && startSession(s.session_id)}
                title={s.session_id}
              >
                <div className="session-row">
                  <div className="session-title">{s.title}</div>
                  <div className="session-actions">
                    <span
                      title={t.renameSession}
                      onClick={async (e) => {
                        e.stopPropagation();
                        const title = window.prompt(t.renameSession, s.title);
                        if (!title?.trim()) return;
                        try {
                          if (!sessionId) await startSession();
                          await invoke("agent_session_rename", {
                            sessionId: s.session_id,
                            title: title.trim(),
                            workspace,
                          });
                          refreshSessions(workspace);
                        } catch (err) {
                          setError(String(err));
                        }
                      }}
                    >
                      ✏
                    </span>
                    <span
                      title={t.deleteSession}
                      onClick={async (e) => {
                        e.stopPropagation();
                        if (!window.confirm(t.deleteConfirm(s.title))) return;
                        try {
                          if (!sessionId) await startSession();
                          await invoke("agent_session_delete", {
                            sessionId: s.session_id,
                            workspace,
                          });
                          if (s.session_id === sessionId) {
                            setSessionId("");
                            setItems([]);
                          }
                          refreshSessions(workspace);
                        } catch (err) {
                          setError(String(err));
                        }
                      }}
                    >
                      🗑
                    </span>
                  </div>
                </div>
                <div className="session-meta">
                  {s.updated_at.slice(0, 16).replace("T", " ")} · {s.num_messages} {t.messagesUnit}
                  {s.model_id ? ` · ${s.model_id}` : ""}
                </div>
              </div>
            ))}
          </div>
          </>
          )}
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

      {planSteps.length > 0 && (
        <div className="plan-panel">
          <div className="plan-head">📋 {t.planTitle}</div>
          {planSteps.map((p, i) => (
            <div key={i} className={`plan-step ${p.status ?? ""}`}>
              <span className="plan-mark">
                {p.status === "completed" ? "✅" : p.status === "in_progress" ? "▶" : "○"}
              </span>
              <span>{p.content}</span>
            </div>
          ))}
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
          const inlinePerm =
            permission && permission.toolCallId && permission.toolCallId === it.call.toolCallId
              ? permission
              : null;
          return (
            <div key={i} className={`msg tool ${inlinePerm ? "awaiting" : ""}`}>
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
              {inlinePerm && (
                <div className="inline-approval">
                  <span className="inline-approval-label">🔐 {t.needApproval}</span>
                  {inlinePerm.options.map((o) => (
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
          items.some((it) => it.kind === "tool" && it.call.toolCallId === permission.toolCallId)
        ) && (
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

      {showTerminal && (
        <div className="terminal-panel">
          <div className="terminal-head">
            <span>▸ {lang === "zh" ? "终端" : "Terminal"}</span>
            <div>
              <button className="ghost small" title={lang === "zh" ? "清空" : "Clear"} onClick={() => setTerminalLines([])}>
                🧹
              </button>
              <button className="ghost small" onClick={() => setShowTerminal(false)}>
                ✕
              </button>
            </div>
          </div>
          <pre className="terminal-body">
            {terminalLines.length ? terminalLines.join("\n") : lang === "zh" ? "（暂无命令输出）" : "(no command output yet)"}
          </pre>
        </div>
      )}

      <footer className="composer">
        <div className="composer-input-wrap">
          {pastedImages.length > 0 && (
            <div className="image-strip">
              {pastedImages.map((im, i) => (
                <div key={i} className="image-thumb">
                  <img src={im.preview} alt="" />
                  <button
                    title={t.removeImage}
                    onClick={() => setPastedImages((prev) => prev.filter((_, j) => j !== i))}
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
          )}
          {popup && popupItems.length > 0 && (
            <div className="mention-popup">
              {popupItems.map((it, idx) => (
                <div
                  key={it.label}
                  className={`mention-item ${idx === popup.sel ? "active" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    acceptPopup(idx);
                  }}
                >
                  <span className="mention-label">{it.label}</span>
                  {it.desc && <span className="mention-desc">{it.desc}</span>}
                </div>
              ))}
            </div>
          )}
          <textarea
            ref={taRef}
            value={input}
            onChange={(e) => onComposerChange(e.currentTarget.value)}
            onPaste={onPaste}
            onKeyDown={(e) => {
              if (popup && popupItems.length > 0) {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setPopup({ ...popup, sel: (popup.sel + 1) % popupItems.length });
                  return;
                }
                if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setPopup({ ...popup, sel: (popup.sel - 1 + popupItems.length) % popupItems.length });
                  return;
                }
                if (e.key === "Enter" || e.key === "Tab") {
                  e.preventDefault();
                  acceptPopup(popup.sel);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setPopup(null);
                  return;
                }
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            placeholder={
              sessionId ? t.composerPlaceholder + "  ·  @文件  /命令" : t.composerLocked
            }
            disabled={!sessionId}
            rows={3}
          />
        </div>
        {sessionId && (
          <button
            className="ghost term-toggle"
            title={lang === "zh" ? "终端" : "Terminal"}
            onClick={() => setShowTerminal((s) => !s)}
          >
            ▸_
          </button>
        )}
        {busy ? (
          <button
            className="send stop"
            onClick={() => invoke("agent_cancel").catch(() => {})}
            title={t.stopTitle}
          >
            {t.stop}
          </button>
        ) : (
          <button className="send" onClick={send} disabled={!sessionId || (!input.trim() && pastedImages.length === 0)}>
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
