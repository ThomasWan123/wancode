import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { STRINGS, loadLang, saveLang, type Lang } from "./i18n";
import {
  IconFolder, IconSettings, IconSun, IconMoon, IconRewind, IconGitBranch,
  IconClipboard, IconTerminal, IconArrowUp, IconStop, IconPlus,
  IconX, IconPencil, IconTrash, IconFile, IconFolderClosed,
  IconCheck, IconShield, IconChevron, IconSearch,
} from "./icons";
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
              <span className="tree-icon">{isDir ? <IconFolderClosed size={13} /> : <IconFile size={13} />}</span>
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
  | { kind: "note"; text: string }
  | { kind: "tool"; call: ToolCallInfo };

type PermissionRequest = {
  id: number;
  title: string;
  toolCallId?: string;
  options: { optionId: string; name: string; kind: string }[];
};

// 权限模式 —— 决定"何时需要你点批准"，对标 Claude Code 的 Mode 菜单。
// engineMode: 传给引擎的会话模式（plan=只读；其余=default）。
type PermMode = "manual" | "acceptEdits" | "plan" | "auto" | "bypass";
const MODE_ORDER: PermMode[] = ["manual", "acceptEdits", "plan", "auto", "bypass"];
const MODE_ENGINE: Record<PermMode, string> = {
  manual: "default",
  acceptEdits: "default",
  plan: "plan",
  auto: "default",
  bypass: "default",
};

// A tool call whose side effect is a file edit/write (vs. execute/delete/read).
function isEditKind(kind: string): boolean {
  return /edit|write|create|patch|modify|move|append/.test(kind.toLowerCase());
}
// Pick an option by its ACP PermissionOptionKind (allow_once / allow_always …).
function pickOption(
  options: { optionId: string; kind: string }[],
  re: RegExp,
): string | undefined {
  return options.find((o) => re.test((o.kind || "").toLowerCase()))?.optionId;
}
// Given the mode + the tool kind + offered options, decide the option to
// auto-select, or null to fall through to an interactive prompt.
function autoApproveOption(
  mode: PermMode,
  toolKind: string,
  options: { optionId: string; kind: string }[],
): string | null {
  const once = () => pickOption(options, /allow[_ ]?once/) ?? pickOption(options, /allow/);
  const always = () => pickOption(options, /allow[_ ]?always/) ?? once();
  switch (mode) {
    case "acceptEdits":
      return isEditKind(toolKind) ? once() ?? null : null;
    case "auto":
      return once() ?? null;
    case "bypass":
      return always() ?? null;
    default: // manual, plan → always prompt
      return null;
  }
}

// 首页建议：从**真实工作区**推导，而不是写死一串示例。
// 之前固定的 "读取 notes.md…" 在多数项目里指向并不存在的文件。
function buildSuggestions(
  files: string[],
  git: any,
  t: any,
): { label: string; prompt: string }[] {
  const out: { label: string; prompt: string }[] = [];
  const lower = files.map((f) => f.toLowerCase());
  const hasReadme = lower.some((f) => f === "readme.md" || f.endsWith("/readme.md"));
  const hasTests = lower.some(
    (f) => /(^|\/)(tests?|__tests__|spec)\//.test(f) || /\.(test|spec)\.[a-z]+$/.test(f),
  );
  const dirty = git?.isRepo ? (git.files?.length ?? 0) : 0;

  if (dirty > 0) {
    out.push({ label: t.sugReviewChanges, prompt: t.sugReviewChangesP });
    out.push({ label: t.sugCommitMsg, prompt: t.sugCommitMsgP });
  }
  if (hasReadme) out.push({ label: t.sugSummarize, prompt: t.sugSummarizeP });
  if (hasTests) out.push({ label: t.sugRunTests, prompt: t.sugRunTestsP });
  out.push({ label: t.sugExplainStruct, prompt: t.sugExplainStructP });
  out.push({ label: t.sugFindBugs, prompt: t.sugFindBugsP });
  return out.slice(0, 4);
}

// ── 组件 ─────────────────────────────────────────────────────────

function App() {
  const [workspace, setWorkspace] = useState(localStorage.getItem("wancode-workspace") || "");
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
  const [showSearch, setShowSearch] = useState(false);
  const [planSteps, setPlanSteps] = useState<{ content: string; status?: string }[]>([]);
  const [gitInfo, setGitInfo] = useState<any>(null);
  const [showGit, setShowGit] = useState(false);
  const [pastedImages, setPastedImages] = useState<{ data: string; mime: string; preview: string }[]>([]);
  const [permMode, setPermMode] = useState<PermMode>(
    (localStorage.getItem("wancode-perm-mode") as PermMode) || "manual",
  );
  const permModeRef = useRef<PermMode>(permMode);
  const [modeMenu, setModeMenu] = useState(false);
  const [plusMenu, setPlusMenu] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const didAutoStart = useRef(false);
  const sessionIdRef = useRef("");

  // Launch straight into a session (last-used folder, else home) so you can
  // type immediately — no forced folder pick, like Claude Code / Codex in cwd.
  useEffect(() => {
    if (didAutoStart.current) return;
    didAutoStart.current = true;
    (async () => {
      let ws = workspace;
      if (!ws) {
        try {
          ws = await invoke<string>("default_workspace");
        } catch {
          ws = "";
        }
        if (ws) setWorkspace(ws);
      }
      if (ws) startSession(undefined, ws);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function pickFolderAndConnect() {
    setPlusMenu(false);
    const dir = await openDialog({ directory: true, title: t.pickFolderTitle });
    if (typeof dir === "string" && dir) {
      setWorkspace(dir);
      localStorage.setItem("wancode-workspace", dir);
      refreshSessions(dir);
      // Auto-open the workspace (start the session) — one action, Claude-style.
      startSession(undefined, dir);
    }
  }

  function onPickImages(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? []).filter((f) => f.type.startsWith("image/"));
    for (const file of files) {
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = String(reader.result);
        const base64 = dataUrl.split(",")[1] ?? "";
        setPastedImages((prev) => [...prev, { data: base64, mime: file.type, preview: dataUrl }]);
      };
      reader.readAsDataURL(file);
    }
    e.target.value = "";
  }
  const [planApproval, setPlanApproval] = useState<{ id: number; planContent: string } | null>(null);
  const [planFeedback, setPlanFeedback] = useState("");
  const [skills, setSkills] = useState<{ name: string; description: string; path: string }[]>([]);
  const [skillForm, setSkillForm] = useState({ name: "", description: "" });
  const [editingSkill, setEditingSkill] = useState<{ name: string; content: string } | null>(null);
  const [migrateMsg, setMigrateMsg] = useState("");

  async function openSkillEditor(name: string) {
    try {
      const content = await invoke<string>("skill_read", { name });
      setEditingSkill({ name, content });
    } catch (e) {
      setError(String(e));
    }
  }

  async function respondPlan(outcome: string) {
    if (!planApproval) return;
    const id = planApproval.id;
    const fb = planFeedback.trim() || null;
    setPlanApproval(null);
    setPlanFeedback("");
    await invoke("agent_plan_respond", { id, outcome, feedback: fb }).catch((e) => setError(String(e)));
  }

  async function setMode(next: PermMode) {
    const prev = permModeRef.current;
    setPermMode(next);
    permModeRef.current = next;
    localStorage.setItem("wancode-perm-mode", next);
    // Only the plan/default engine switch needs a round-trip; the auto-approval
    // policy (acceptEdits/auto/bypass) lives entirely on the client.
    if (sessionId && MODE_ENGINE[next] !== MODE_ENGINE[prev]) {
      await invoke("agent_set_mode", { mode: MODE_ENGINE[next] }).catch((e) => {
        setError(String(e));
        setPermMode(prev);
        permModeRef.current = prev;
        localStorage.setItem("wancode-perm-mode", prev);
      });
    }
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
          // Engine only knows plan vs. default; reconcile without clobbering the
          // client-only auto-approval modes (acceptEdits/auto/bypass).
          if (m === "plan" && permModeRef.current !== "plan") {
            setPermMode("plan");
            permModeRef.current = "plan";
          } else if (m && m !== "plan" && permModeRef.current === "plan") {
            setPermMode("manual");
            permModeRef.current = "manual";
          }
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
        const title = p.request?.toolCall?.title ?? p.request?.toolCall?.kind ?? "工具调用请求";
        const toolKind = String(p.request?.toolCall?.kind ?? "");
        const options = (p.request?.options ?? []).map((o: any) => ({
          optionId: o.optionId,
          name: o.name,
          kind: o.kind,
        }));
        // Permission-mode policy: auto-approve without prompting when the
        // current mode allows it (acceptEdits/auto/bypass). Manual/plan prompt.
        const auto = autoApproveOption(permModeRef.current, toolKind, options);
        if (auto) {
          invoke("agent_permission_respond", { id: p.id, optionId: auto }).catch(() => {});
          setItems((prev) => [...prev, { kind: "note", text: STRINGS[loadLang()].modeAutoApproved(title) }]);
          return;
        }
        setPermission({
          id: p.id,
          title,
          toolCallId: p.request?.toolCall?.toolCallId,
          options,
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

  async function startSession(resume?: string, ws?: string): Promise<string> {
    const wsPath = ws ?? workspace;
    setStarting(true);
    setError("");
    setItems([]);
    setSessionId("");
    sessionIdRef.current = "";
    try {
      const r = await invoke<{ session_id: string; models: string[] }>("agent_start", {
        workspace: wsPath,
        model,
        resume: resume ?? null,
      });
      setSessionId(r.session_id);
      sessionIdRef.current = r.session_id;
      if (r.models?.length) setModels(r.models);
      refreshSessions(wsPath);
      invoke<string[]>("list_workspace_files", { workspace: wsPath })
        .then(setFileList)
        .catch(() => {});
      refreshGit();
      return r.session_id;
    } catch (e) {
      setError(String(e));
      return "";
    } finally {
      setStarting(false);
    }
  }

  function sendText(text: string, imgs: { data: string; mime: string; preview: string }[] = []) {
    const t = text.trim();
    if ((!t && imgs.length === 0) || busy || !sessionIdRef.current) return;
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

  async function send() {
    if (popup) return;
    const text = input.trim();
    if (!text && pastedImages.length === 0) return;
    // Typed before the session was ready — spin one up first, then send.
    if (!sessionId) {
      if (!starting) {
        const ws = workspace || (await invoke<string>("default_workspace").catch(() => ""));
        if (ws) {
          if (ws !== workspace) setWorkspace(ws);
          const sid = await startSession(undefined, ws);
          if (!sid) return; // start failed; keep the text so the user can retry
        } else {
          return;
        }
      } else {
        return; // a start is already in flight; user can press Enter again
      }
    }
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
          sessionIdRef.current = "";
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

  const modeMeta: Record<PermMode, { label: string; desc: string }> = {
    manual: { label: t.modeManual, desc: t.modeManualDesc },
    acceptEdits: { label: t.modeAcceptEdits, desc: t.modeAcceptEditsDesc },
    plan: { label: t.modePlan, desc: t.modePlanDesc },
    auto: { label: t.modeAuto, desc: t.modeAutoDesc },
    bypass: { label: t.modeBypass, desc: t.modeBypassDesc },
  };

  return (
    <main className="chat-app">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark">W</div>
          <div className="brand-name">WanCode</div>
        </div>
        <div style={{ flex: 1 }} />
        {sessionId && (
          <span className="connected-pill">
            <span className="dot" />
            {t.connected}
          </span>
        )}
        {sessionId && (
          <button className="icon-btn" title={t.rewindTooltip} onClick={openRewind}>
            <IconRewind />
          </button>
        )}
        {sessionId && (
          <button
            className="icon-btn"
            title={t.git}
            onClick={() => {
              refreshGit();
              setShowGit(true);
            }}
          >
            <IconGitBranch />
          </button>
        )}
        <button
          className="icon-btn"
          title={t.toggleTheme}
          onClick={() => setTheme((th) => (th === "dark" ? "light" : "dark"))}
        >
          {theme === "dark" ? <IconSun /> : <IconMoon />}
        </button>
        <button
          className="icon-btn"
          title={t.settings}
          onClick={() => {
            refreshMcpConfig();
            setShowSettings(true);
          }}
        >
          <IconSettings />
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
                      <IconX size={13} />
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
              <div style={{ marginTop: 12, display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                <button
                  className="ghost"
                  onClick={async () => {
                    try {
                      const n = await invoke<number>("migrate_env_keys");
                      setMigrateMsg(t.migrateOk(n));
                      refreshModels();
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t.migrateKeys}
                </button>
                {migrateMsg && <span className="model-test-msg" style={{ margin: 0 }}>{migrateMsg}</span>}
              </div>
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
                      <IconX size={13} />
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
                  <div
                    key={sk.name}
                    className="mcp-item clickable"
                    onClick={() => openSkillEditor(sk.name)}
                  >
                    <div className="mcp-info">
                      <b>{sk.name}</b>
                      <span className="mcp-detail">{sk.description}</span>
                    </div>
                    <IconPencil size={13} className="skill-edit-icon" />
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
                      <IconX size={13} />
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
            <div className="modal-title panel-title"><IconGitBranch size={16} /> {t.git}</div>
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
                      await invoke("skill_write", { name: editingSkill.name, content: editingSkill.content });
                      setEditingSkill(null);
                      setSkills(await invoke("skills_list"));
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

      <div className="body-row">
        <aside className="sidebar">
          {/* 新会话置顶 + 导航 + 最近 —— 层次对标 Claude Code 左栏 */}
          <button
            className="side-new"
            onClick={() => {
              setSessionId("");
              sessionIdRef.current = "";
              setItems([]);
              setSidebarTab("sessions");
            }}
          >
            <IconPlus size={15} /> {t.sidebarNewSession}
          </button>

          <nav className="side-nav">
            <button
              className={`side-nav-item ${sidebarTab === "files" ? "active" : ""}`}
              onClick={() => setSidebarTab(sidebarTab === "files" ? "sessions" : "files")}
            >
              <IconFolderClosed size={15} /> {t.tabFiles}
            </button>
            <button
              className="side-nav-item"
              onClick={async () => {
                setSettingsTab("skills");
                setShowSettings(true);
                setSkills(
                  await invoke<{ name: string; description: string; path: string }[]>(
                    "skills_list",
                  ).catch(() => []),
                );
              }}
            >
              <IconClipboard size={15} /> {t.navSkills}
            </button>
            <button
              className="side-nav-item"
              onClick={() => {
                refreshMcpConfig();
                setSettingsTab("mcp");
                setShowSettings(true);
              }}
            >
              <IconGitBranch size={15} /> {t.navMcp}
              {mcpServers.length > 0 && <span className="side-nav-count">{mcpServers.length}</span>}
            </button>
            <button
              className="side-nav-item"
              onClick={() => {
                setSettingsTab("general");
                setShowSettings(true);
              }}
            >
              <IconSettings size={15} /> {t.settings}
            </button>
          </nav>

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
          <div className="side-section">
            <span className="side-section-title">{t.sidebarRecent}</span>
            <button
              className="icon-btn side-section-btn"
              title={t.sidebarSearchToggle}
              onClick={() => {
                const next = !showSearch;
                setShowSearch(next);
                if (!next) runSearch("");
              }}
            >
              <IconSearch size={14} />
            </button>
          </div>
          {showSearch && (
            <input
              className="session-search"
              autoFocus
              value={searchQuery}
              placeholder={t.searchPlaceholder}
              onChange={(e) => runSearch(e.currentTarget.value)}
            />
          )}
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
                      <IconPencil size={13} />
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
                            sessionIdRef.current = "";
                            setItems([]);
                          }
                          refreshSessions(workspace);
                        } catch (err) {
                          setError(String(err));
                        }
                      }}
                    >
                      <IconTrash size={13} />
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
          {/* 底部工作区状态行（对应 Claude Code 左栏底部的账号行） */}
          <div className="side-foot">
            <button
              className="side-foot-ws"
              onClick={pickFolderAndConnect}
              disabled={starting}
              title={workspace || t.sugOpenFolder}
            >
              <IconFolder size={14} />
              <span className="side-foot-name">
                {workspace ? workspace.split(/[\\/]/).filter(Boolean).pop() : t.sugOpenFolder}
              </span>
            </button>
            <div className="side-foot-meta">
              {gitInfo?.isRepo ? (
                <>
                  <IconGitBranch size={11} /> {gitInfo.branch}
                  {(gitInfo.files?.length ?? 0) > 0 && ` · ${t.homeChanged(gitInfo.files.length)}`}
                </>
              ) : workspace ? (
                t.sidebarNoRepo
              ) : null}
            </div>
          </div>
        </aside>

        <div className="main-col">
      {items.length === 0 && !busy && (
        <div className="empty-state">
          <div className="empty-logo">W</div>
          <div className="empty-title">{t.appTagline}</div>

          {/* 建议来自当前工作区（有改动就先建议审查改动，有 README 才建议总结…）
              工作区信息不在这里重复 —— 左栏底部和输入框上方已经显示。 */}
          {sessionId && (
            <div className="chips">
              {buildSuggestions(fileList, gitInfo, t).map((s) => (
                <button
                  key={s.label}
                  className="chip"
                  onClick={() => {
                    setInput(s.prompt);
                    onComposerChange(s.prompt);
                    taRef.current?.focus();
                  }}
                >
                  {s.label}
                </button>
              ))}
            </div>
          )}

          <div className="empty-hint">{t.emptyHint}</div>
        </div>
      )}

      {planSteps.length > 0 && (
        <div className="plan-panel">
          <div className="plan-head"><IconClipboard size={14} /> {t.planTitle}</div>
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
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{it.text}</ReactMarkdown>
              </div>
            );
          if (it.kind === "thought")
            return (
              <details key={i} className="msg thought">
                <summary>{t.thinking}</summary>
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{it.text}</ReactMarkdown>
              </details>
            );
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
              {it.call.diffs.map((d, j) => (
                <DiffView key={j} diff={d} />
              ))}
              {it.call.output && (
                <details className="tool-result">
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
            <span className="panel-title"><IconTerminal size={14} /> {lang === "zh" ? "终端" : "Terminal"}</span>
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

      <input
        ref={fileInputRef}
        type="file"
        accept="image/*"
        multiple
        style={{ display: "none" }}
        onChange={onPickImages}
      />
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
                    <IconX size={12} />
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
            placeholder={sessionId ? t.composerPlaceholder : starting ? t.starting : t.composerHint}
            rows={2}
          />
          <div className="composer-bar">
            <div className="composer-left">
              <div className="plus-wrap">
                <button
                  className="icon-btn plus-btn"
                  title={t.addMenu}
                  onClick={() => setPlusMenu((v) => !v)}
                >
                  <IconPlus size={18} />
                </button>
                {plusMenu && (
                  <>
                    <div className="plus-backdrop" onClick={() => setPlusMenu(false)} />
                    <div className="plus-menu">
                      <button className="plus-item" onClick={pickFolderAndConnect}>
                        <IconFolder size={15} /> {t.menuOpenFolder}
                      </button>
                      <button
                        className="plus-item"
                        disabled={!sessionId}
                        onClick={() => {
                          setPlusMenu(false);
                          fileInputRef.current?.click();
                        }}
                      >
                        <IconFile size={15} /> {t.menuAddImage}
                      </button>
                      <button
                        className="plus-item"
                        disabled={!sessionId}
                        onClick={() => {
                          setPlusMenu(false);
                          setInput("/");
                          onComposerChange("/");
                          taRef.current?.focus();
                        }}
                      >
                        <IconClipboard size={15} /> {t.menuSlash}
                      </button>
                      <button
                        className="plus-item"
                        onClick={() => {
                          setPlusMenu(false);
                          refreshMcpConfig();
                          setSettingsTab("mcp");
                          setShowSettings(true);
                        }}
                      >
                        <IconGitBranch size={15} /> {t.menuMcp}
                      </button>
                    </div>
                  </>
                )}
              </div>
              {sessionId ? (
                <span className="ws-inline" title={workspace}>
                  <span className="dot" />
                  {workspace.split(/[\\/]/).filter(Boolean).pop()}
                </span>
              ) : (
                <button className="ws-inline connect" onClick={pickFolderAndConnect} disabled={starting}>
                  <IconFolder size={13} />
                  {starting ? t.starting : t.openWorkspace}
                </button>
              )}
              <select
                className="composer-model"
                value={model}
                title={t.modelSwitchHint}
                onChange={(e) => {
                  const m = e.currentTarget.value;
                  setModel(m);
                  // Live switch — no restart, keeps conversation context.
                  if (sessionId) invoke("agent_set_model", { model: m }).catch((err) => setError(String(err)));
                }}
              >
                {(models.length ? models : ["glm-5.2", "glm-5-turbo", "glm-4-flash", "deepseek-chat", "deepseek-reasoner"]).map(
                  (m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ),
                )}
              </select>
              <div className="mode-wrap">
                <button
                  className="mode-chip"
                  data-mode={permMode}
                  title={t.modeMenuTitle}
                  onClick={() => setModeMenu((v) => !v)}
                >
                  <IconShield size={13} /> {modeMeta[permMode].label}
                  <IconChevron size={12} />
                </button>
                {modeMenu && (
                  <>
                    <div className="plus-backdrop" onClick={() => setModeMenu(false)} />
                    <div className="mode-menu">
                      <div className="mode-menu-head">{t.modeMenuTitle}</div>
                      {MODE_ORDER.map((m) => (
                        <button
                          key={m}
                          className={`mode-item ${permMode === m ? "active" : ""}`}
                          data-mode={m}
                          onClick={() => {
                            setModeMenu(false);
                            setMode(m);
                          }}
                        >
                          <span className="mode-item-text">
                            <span className="mode-item-label">{modeMeta[m].label}</span>
                            <span className="mode-item-desc">{modeMeta[m].desc}</span>
                          </span>
                          {permMode === m && <IconCheck size={15} className="mode-item-check" />}
                        </button>
                      ))}
                    </div>
                  </>
                )}
              </div>
            </div>
            <div className="composer-actions">
              {sessionId && (
                <button
                  className="icon-btn"
                  title={lang === "zh" ? "终端" : "Terminal"}
                  onClick={() => setShowTerminal((s) => !s)}
                >
                  <IconTerminal size={15} />
                </button>
              )}
              {busy ? (
                <button
                  className="send-btn stop"
                  onClick={() => invoke("agent_cancel").catch(() => {})}
                  title={t.stopTitle}
                >
                  <IconStop size={16} />
                </button>
              ) : (
                <button
                  className="send-btn"
                  onClick={send}
                  disabled={starting || (!input.trim() && pastedImages.length === 0)}
                  title={t.send}
                >
                  <IconArrowUp size={16} />
                </button>
              )}
            </div>
          </div>
        </div>
      </footer>
        </div>
      </div>
    </main>
  );
}

export default App;
