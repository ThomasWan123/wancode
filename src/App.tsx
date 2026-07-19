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
  IconCheck, IconShield, IconChevron, IconSearch, IconCopy,
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
// 引擎的 ChangeType 是 create/edit/delete/... —— 映射成 git 用户熟悉的字母。
const CHANGE_LETTER: Record<string, string> = {
  create: "A",
  edit: "M",
  delete: "D",
  rename: "R",
  copy: "C",
  typechange: "T",
  untracked: "?",
};
function changeLetter(t: unknown): string {
  return CHANGE_LETTER[String(t ?? "").toLowerCase()] ?? "M";
}

const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

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
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  const [compacting, setCompacting] = useState(false);
  const [engineCommands, setEngineCommands] = useState<{ name: string; description?: string }[]>([]);
  // 引擎权威队列（x.ai/queue/changed 广播），不含正在跑的那条
  const [queue, setQueue] = useState<{ id: string; version: number; text: string }[]>([]);
  // 引擎主动发起的提问（x.ai/ask_user_question）
  const [question, setQuestion] = useState<{
    id: number;
    questions: {
      question: string;
      options: { label: string; description?: string }[];
      multiSelect?: boolean;
    }[];
  } | null>(null);
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  // ↑ 调取历史输入
  const historyRef = useRef<string[]>([]);
  const histIdxRef = useRef(-1);
  const draftRef = useRef("");
  // 展开过的思考块索引；新回合开始时清空 —— 读完的思考不会一直摊在记录里
  const [openThoughts, setOpenThoughts] = useState<Set<number>>(new Set());
  const [planSteps, setPlanSteps] = useState<{ content: string; status?: string }[]>([]);
  const [gitInfo, setGitInfo] = useState<any>(null);
  const [showGit, setShowGit] = useState(false);
  const [gitBranches, setGitBranches] = useState<string[]>([]);
  const [bgTasks, setBgTasks] = useState<any[]>([]);
  const [subagents, setSubagents] = useState<any[]>([]);
  const [showTasks, setShowTasks] = useState(false);
  const [otherRecent, setOtherRecent] = useState<
    { sessionId: string; path: string; title: string; branch: string; updatedAt: string }[]
  >([]);
  // 定时任务：引擎不提供 list，只能从通知流重建（见 applySchedUpdate）
  const [schedTasks, setSchedTasks] = useState<
    Record<string, { taskId: string; prompt: string; humanSchedule: string; nextFireAt?: string }>
  >({});
  const [mcpLive, setMcpLive] = useState<any[]>([]);
  const [knownWorkspaces, setKnownWorkspaces] = useState<
    { path: string; sessions: number; updatedAt: string }[]
  >([]);
  const [wsMenu, setWsMenu] = useState(false);
  const [grepQuery, setGrepQuery] = useState("");
  const [grepHits, setGrepHits] = useState<
    { path: string; name: string; matches: { line: number; content: string }[] }[] | null
  >(null);
  const [grepping, setGrepping] = useState(false);
  const [trustReq, setTrustReq] = useState<{
    id: number;
    workspace: string;
    cwd: string;
    configKinds: string[];
  } | null>(null);
  const [commitMsg, setCommitMsg] = useState("");
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

  // 设置页的数据按"当前分类"加载，而不是挂在某个入口按钮上 ——
  // 否则从齿轮进来再切到 MCP/技能，面板会是空的（数据从没被拉过）。
  useEffect(() => {
    if (!showSettings) return;
    if (settingsTab === "mcp") {
      refreshMcpConfig();
      refreshMcpLive();
    } else if (settingsTab === "skills") {
      invoke<{ name: string; description: string; path: string }[]>("skills_list")
        .then(setSkills)
        .catch(() => {});
    } else if (settingsTab === "models") {
      refreshModels();
    } else if (settingsTab === "hooks") {
      refreshMcpConfig(); // 它同时加载 hooks_list（函数名没体现）
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showSettings, settingsTab, sessionId]);

  async function runGrep() {
    const q = grepQuery.trim();
    if (!q || !sessionIdRef.current) {
      setGrepHits(null);
      return;
    }
    setGrepping(true);
    setError("");
    try {
      const r = await invoke<any>("search_content", { pattern: q });
      if (r?.error) throw new Error(typeof r.error === "string" ? r.error : JSON.stringify(r.error));
      const env = r?.result ?? r;
      setGrepHits(env?.files ?? env?.matches ?? []);
    } catch (e) {
      setError(String(e));
      setGrepHits([]);
    } finally {
      setGrepping(false);
    }
  }

  // 有过会话历史的工作区（引擎按 cwd 分组），用于跨项目切换。
  async function refreshWorkspaces() {
    if (!sessionIdRef.current) return;
    try {
      setKnownWorkspaces(await invoke("workspace_list"));
    } catch {
      /* 拿不到就只保留"浏览文件夹" */
    }
  }

  // 首页的“别的项目干到哪了”。当前工作区排除掉——左栏已经列了它的会话。
  async function refreshOtherRecent(ws: string) {
    if (!sessionIdRef.current) return;
    try {
      const all = await invoke<any[]>("recent_sessions", { limit: 30 });
      const cur = (ws || "").replace(/[\\/]+$/, "").toLowerCase();
      // 每个项目只留最近一条：这一栏回答的是“哪些项目还有活儿”，
      // 同一个项目占掉三行只会把其他项目挤掉。
      const seen = new Set<string>();
      const perProject = [];
      for (const s of all) {
        const key = (s.path || "").replace(/[\\/]+$/, "").toLowerCase();
        if (key === cur || seen.has(key)) continue;
        seen.add(key);
        perProject.push(s);
        if (perProject.length === 4) break;
      }
      setOtherRecent(perProject);
    } catch (e) {
      setError(`recent: ${String(e)}`);
    }
  }

  // MCP 实时状态。fresh=true 绕过缓存（OAuth 授权/断开之后必须这样拉）。
  async function refreshMcpLive(fresh = false) {
    if (!sessionIdRef.current) {
      setMcpLive([]);
      return;
    }
    try {
      const r = await invoke<any>("mcp_live_list", { fresh });
      setMcpLive((r?.result ?? r)?.servers ?? []);
    } catch (e) {
      setMcpLive([]);
    }
  }

  // 引擎会推 task_backgrounded / task_completed 通知；收到就重新拉一次。
  async function refreshTasks() {
    if (!sessionIdRef.current) {
      setBgTasks([]);
      setSubagents([]);
      return;
    }
    try {
      const t = await invoke<any>("tasks_list");
      setBgTasks((t?.result ?? t)?.tasks ?? []);
    } catch {
      setBgTasks([]);
    }
    try {
      const s = await invoke<any>("subagents_list");
      setSubagents((s?.result ?? s)?.subagents ?? []);
    } catch {
      setSubagents([]);
    }
  }

  // 走引擎的 workspace ops：它已处理 gitRoot 解析 / worktree / 子模块，
  // 也不会像我们自己 shell 调 git 那样在 Windows 上闪控制台。
  async function refreshGit() {
    if (!sessionIdRef.current) {
      setGitInfo(null);
      return;
    }
    try {
      const r = await invoke<any>("git_status_ext");
      // 引擎统一包成 { result: T | null, error? }。失败时 result 为 null ——
      // 必须区分"引擎报错"和"这不是 git 仓库"，否则会把故障伪装成正常状态。
      if (r?.error) {
        setGitInfo(null);
        setError(`git: ${typeof r.error === "string" ? r.error : JSON.stringify(r.error)}`);
        return;
      }
      // 兼容 {result:{format,data}} / {format,data} / 旧版扁平 GitStatusData
      const env = r?.result ?? r;
      const d =
        env?.data ??
        (env && (env.branch !== undefined || env.staged !== undefined || env.root !== undefined)
          ? env
          : null);
      if (!d) {
        setGitInfo({ isRepo: false });
        return;
      }
      setGitInfo({
        isRepo: true,
        branch: d.branch ?? "",
        ahead: d.ahead ?? 0,
        behind: d.behind ?? 0,
        staged: d.staged ?? [],
        unstaged: d.unstaged ?? [],
        // 底部状态行只关心"有多少改动"
        files: [...(d.staged ?? []), ...(d.unstaged ?? [])],
      });
    } catch {
      setGitInfo(null);
    }
  }

  async function gitOp(cmd: string, args: Record<string, unknown> = {}) {
    setError("");
    try {
      await invoke(cmd, args);
      await refreshGit();
    } catch (e) {
      setError(String(e));
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
    // 引擎注册表里的命令（技能 / 插件 / 内置），本地已有的同名项不重复列出。
    // 直接把 "/name" 作为 prompt 发给引擎，由它自己解析执行。
    ...engineCommands
      .filter((c) => c?.name)
      .map((c) => ({
        cmd: `/${c.name}`,
        desc: c.description ?? "",
        prompt: `/${c.name}`,
      }))
      .filter(
        (c) =>
          !["/commit", "/review", "/test", "/explain", "/compact", "/rewind", "/clear"].includes(
            c.cmd,
          ),
      ),
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
  // 分叉回放上限：只在 forkFrom 设置，null 表示正常会话不限制
  const replayCapRef = useRef<number | null>(null);
  const replaySuppressRef = useRef(false);
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
        // 恢复会话时定时任务是通过重播 updates.jsonl 回来的，走这条路而不是
        // ext 通知——两条都接才不会“重开应用定时任务就没了”。
        if (typeof u.sessionUpdate === "string" && u.sessionUpdate.startsWith("scheduled_task_")) {
          applySchedUpdate(u);
          return;
        }
        // 分叉回放超出上限后，这一轮剩下的内容（回复/工具调用）一并丢弃，
        // 直到用户真正发出下一条消息为止（sendText 会清掉这个标记）。
        if (replaySuppressRef.current) return;
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
          } else if (replayCapRef.current !== null) {
            // 分叉回放：只放行模型上下文里真实存在的那几轮（见 forkFrom）
            if (replayCapRef.current > 0) {
              replayCapRef.current -= 1;
              appendStream("user", u.content.text);
            } else {
              replaySuppressRef.current = true;
            }
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

    // 引擎广播的权威队列。排除正在执行的那条 —— 它已经在对话流里了。
    unsubs.push(
      listen<any>("agent://ext", (e) => {
        const m = e.payload?.method;
        // 后台任务开始/结束 —— 之前这两个通知被直接丢弃
        if (m === "x.ai/task_backgrounded" || m === "x.ai/task_completed") {
          refreshTasks();
          return;
        }
        if (m === "x.ai/mcp/servers_updated" || m === "x.ai/mcp/tools_changed") {
          refreshMcpLive();
          return;
        }
        // 实时定时任务通知（恢复会话时引擎还会整体重播一遍 created）
        if (
          m === "x.ai/scheduled_task_created" ||
          m === "x.ai/scheduled_task_deleted" ||
          m === "x.ai/scheduled_task_fired"
        ) {
          applySchedUpdate(e.payload?.params?.update);
          return;
        }
        if (m !== "x.ai/queue/changed") return;
        const p = e.payload.params ?? {};
        const running = p.runningPromptId;
        setQueue(
          (p.entries ?? [])
            .filter((q: any) => q.id !== running)
            .map((q: any) => ({ id: q.id, version: q.version ?? 0, text: q.text ?? "" })),
        );
      }),
    );

    unsubs.push(
      listen<any>("agent://folder-trust", (e) => {
        setTrustReq({
          id: e.payload.id,
          workspace: e.payload.workspace ?? "",
          cwd: e.payload.cwd ?? "",
          configKinds: e.payload.configKinds ?? [],
        });
      }),
    );

    unsubs.push(
      listen<any>("agent://ask-question", (e) => {
        setQuestion({ id: e.payload.id, questions: e.payload.questions ?? [] });
        setAnswers({});
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
        refreshTasks();
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

  async function startSession(resume?: string, ws?: string, keepReplayCap = false): Promise<string> {
    const wsPath = ws ?? workspace;
    // 普通会话切换必须清掉分叉回放上限，否则它会泄漏到下一个会话把内容吃掉
    if (!keepReplayCap) {
      replayCapRef.current = null;
      replaySuppressRef.current = false;
    }
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
      refreshTasks();
      refreshMcpLive();
      refreshOtherRecent(wsPath);
      // ↑ 历史（引擎按 cwd 维护，最近优先）
      invoke<string[]>("agent_prompt_history", { workspace: wsPath })
        .then((h) => {
          historyRef.current = h;
          histIdxRef.current = -1;
        })
        .catch(() => {});
      // 斜杠命令来自引擎注册表：内置 + 技能 + 插件，而非界面硬编码
      invoke<any>("agent_commands_list", { workspace: wsPath })
        .then((r) => setEngineCommands(r?.commands ?? []))
        .catch(() => {});
      return r.session_id;
    } catch (e) {
      setError(String(e));
      return "";
    } finally {
      setStarting(false);
    }
  }

  async function respondQuestion(send: boolean) {
    if (!question) return;
    const id = question.id;
    const payload = send ? answers : null;
    setQuestion(null);
    setAnswers({});
    await invoke("agent_question_respond", { id, answers: payload }).catch((e) =>
      setError(String(e)),
    );
  }

  function toggleAnswer(q: string, label: string, multi: boolean) {
    setAnswers((prev) => {
      const cur = prev[q] ?? [];
      if (!multi) return { ...prev, [q]: [label] };
      return { ...prev, [q]: cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label] };
    });
  }

  async function runCompact() {
    if (!sessionIdRef.current || compacting) return;
    setCompacting(true);
    setError("");
    try {
      await invoke("agent_compact", { userContext: null });
      setItems((prev) => [...prev, { kind: "note", text: STRINGS[loadLang()].compactDone }]);
      refreshCtx(); // 让上下文条立刻反映释放出的空间
    } catch (e) {
      setError(String(e));
    } finally {
      setCompacting(false);
    }
  }

  /// Fold one `scheduled_task_*` session-update into the scheduled-task view.
  ///
  /// The engine has no scheduler list method, so this notification stream *is*
  /// the source of truth. Two behaviours are load-bearing:
  ///
  ///   - **created must be an upsert, not an insert.** On session restore the
  ///     engine re-announces every live task, and the persisted updates.jsonl
  ///     replays the original created event too — so the same taskId legitimately
  ///     arrives twice.
  ///   - **fired with nextFireAt == null must not self-heal.** That is the
  ///     sentinel for a one-shot that already missed its window and is about to
  ///     be deleted; inserting it would flash a row that vanishes next frame.
  ///
  /// Inner field names are raw snake_case: `rename_all` on the engine's
  /// SessionUpdate enum renames the *variant tag*, not the fields.
  function applySchedUpdate(u: any) {
    const kind = u?.sessionUpdate;
    const id = u?.task_id;
    if (!id) return;
    if (kind === "scheduled_task_deleted") {
      setSchedTasks((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      return;
    }
    if (kind === "scheduled_task_created") {
      setSchedTasks((prev) => ({
        ...prev,
        [id]: {
          taskId: id,
          prompt: u.prompt ?? "",
          humanSchedule: u.human_schedule ?? "",
          nextFireAt: u.next_fire_at ?? undefined,
        },
      }));
      return;
    }
    if (kind === "scheduled_task_fired") {
      setSchedTasks((prev) => {
        if (prev[id]) return { ...prev, [id]: { ...prev[id], nextFireAt: u.next_fire_at ?? undefined } };
        if (!u.next_fire_at) return prev; // missed one-shot, delete is coming
        return {
          ...prev,
          [id]: {
            taskId: id,
            prompt: u.prompt ?? "",
            humanSchedule: u.human_schedule ?? "",
            nextFireAt: u.next_fire_at,
          },
        };
      });
    }
  }

  async function copyMessage(text: string, idx: number) {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedIdx(idx);
      setTimeout(() => setCopiedIdx((c) => (c === idx ? null : c)), 1500);
    } catch (e) {
      setError(String(e));
    }
  }

  /// Branch the conversation at a user message: fork the session keeping only
  /// the turns *before* it, switch to the fork, and put the original text back
  /// in the composer so it can be reworded. (Claude Code's rewind-and-edit.)
  ///
  /// `targetPromptIndex = N` keeps the first N user turns and drops everything
  /// from the (N+1)th onward — so the 0-based ordinal of the clicked message is
  /// exactly the index to pass.
  async function forkFrom(itemIdx: number, text: string) {
    if (!workspace || busy) return;
    const ordinal = items
      .slice(0, itemIdx)
      .filter((x) => x.kind === "user").length;
    setError("");
    try {
      const newId = await invoke<string>("session_fork", {
        workspace,
        targetPromptIndex: ordinal,
      });
      // 引擎里两处截断的约定不一致：chat_history（模型真实上下文）保留 N 轮，
      // 而 updates.jsonl（我们回放界面用的）按 `count > N + 1` 截，多留一轮。
      // 不设这个上限的话，屏幕上会显示一轮模型根本不记得的对话。
      // 必须在 startSession 之前设置——回放事件可能在它返回前就到了。
      replayCapRef.current = ordinal;
      replaySuppressRef.current = false;
      // fork only writes files — the session still has to be opened.
      await startSession(newId, undefined, true);
      setInput(text);
    } catch (e) {
      setError(String(e));
    }
  }

  function sendText(text: string, imgs: { data: string; mime: string; preview: string }[] = []) {
    const t = text.trim();
    if ((!t && imgs.length === 0) || !sessionIdRef.current) return;
    setError("");
    // A turn is already running → the engine appends this to its own FIFO
    // instead of starting a turn. Queued prompts must NOT be echoed locally:
    // they show in the queue strip, and when the engine finally runs one it
    // emits the normal user_message_chunk. Setting lastSentRef here would make
    // that echo get deduped away and the message would vanish.
    // 用户开口了，分叉回放阶段结束
    replayCapRef.current = null;
    replaySuppressRef.current = false;
    const queueing = busy;
    if (!queueing) {
      setOpenThoughts(new Set()); // 新回合：收起上一轮展开过的思考
      lastSentRef.current = t;
      setPlanSteps([]);
      const label = imgs.length ? `${t}${t ? "  " : ""}🖼️×${imgs.length}` : t;
      setItems((prev) => [...prev, { kind: "user", text: label }]);
      setBusy(true);
    }
    // 新发的 prompt 立刻进历史（引擎那份是启动时快照）
    if (t) historyRef.current = [t, ...historyRef.current.filter((h) => h !== t)];
    invoke("agent_prompt", {
      text: t,
      images: imgs.length ? imgs.map((i) => ({ data: i.data, mime: i.mime })) : null,
    }).catch((e) => {
      setError(String(e));
      if (!queueing) setBusy(false);
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
          // 真压缩：走引擎的 compact_conversation 重写历史。
          // 旧实现是给模型发一句"请压缩对话"——那只是又生成一轮回答，
          // 上下文一点没少，反而更满了。
          setInput("");
          runCompact();
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
        {/* 只在真有后台活动时出现 —— 平时不占位置。
            定时任务也要算进来：否则只有定时任务时按钮不出现，面板打不开，
            等于这个功能不存在。 */}
        {sessionId && bgTasks.length + subagents.length + Object.keys(schedTasks).length > 0 && (
          <button
            className="icon-btn tasks-btn"
            title={t.tasksTitle}
            onClick={() => {
              refreshTasks();
              setShowTasks(true);
            }}
          >
            <IconTerminal size={15} />
            <span className="tasks-count">
              {bgTasks.length + subagents.length + Object.keys(schedTasks).length}
            </span>
          </button>
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
          {/* 上下文条本身就是压缩入口 —— 光有仪表盘没有刹车才是问题 */}
          <button
            className="ctx-compact"
            title={t.compactTitle}
            disabled={compacting || busy}
            onClick={runCompact}
          >
            {compacting ? "…" : t.compactTitle}
          </button>
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
              {/* 实时状态：按服务器/按工具启停、授权，全部即时生效，
                  不必再改 TOML 重开会话。 */}
              <div className="modal-label">{t.mcpLiveSection}</div>
              <div className="mcp-list">
                {mcpLive.length === 0 && <div className="sidebar-empty">{t.mcpLiveEmpty}</div>}
                {mcpLive.map((s: any) => {
                  const st = s.session ?? {};
                  const on = st.enabled !== false;
                  return (
                    <div key={s.name} className="mcp-live">
                      <div className="mcp-live-head">
                        <button
                          className={`mcp-switch ${on ? "on" : ""}`}
                          title={on ? t.mcpDisable : t.mcpEnable}
                          onClick={async () => {
                            await invoke("mcp_toggle", {
                              serverName: s.name,
                              enabled: !on,
                            }).catch((e) => setError(String(e)));
                            refreshMcpLive();
                          }}
                        >
                          <span className="mcp-knob" />
                        </button>
                        <span className="mcp-live-name">{s.displayName ?? s.name}</span>
                        {st.status && <span className={`mcp-status ${st.status}`}>{st.status}</span>}
                        {s.sourceLabel && <span className="mcp-src">{s.sourceLabel}</span>}
                        {st.authRequired && (
                          <button
                            className="git-mini"
                            onClick={async () => {
                              await invoke("mcp_auth_trigger", { serverName: s.name }).catch((e) =>
                                setError(String(e)),
                              );
                              refreshMcpLive(true);
                            }}
                          >
                            {t.mcpAuth}
                          </button>
                        )}
                      </div>
                      {on && (st.tools ?? []).length > 0 && (
                        <div className="mcp-tools">
                          {st.tools.map((tool: any) => (
                            <button
                              key={tool.name}
                              className={`mcp-tool ${tool.enabled === false ? "off" : ""}`}
                              title={tool.description ?? tool.name}
                              onClick={async () => {
                                await invoke("mcp_toggle_tool", {
                                  serverName: s.name,
                                  toolName: tool.name,
                                  enabled: tool.enabled === false,
                                }).catch((e) => setError(String(e)));
                                refreshMcpLive();
                              }}
                            >
                              {tool.name}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>

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
                <div className="git-head">
                  <span className="git-branch-name">
                    <IconGitBranch size={13} /> {gitInfo.branch || "-"}
                  </span>
                  {(gitInfo.ahead ?? 0) > 0 && <span className="git-track">↑{gitInfo.ahead}</span>}
                  {(gitInfo.behind ?? 0) > 0 && <span className="git-track">↓{gitInfo.behind}</span>}
                  <select
                    className="git-branch-pick"
                    value=""
                    onChange={(e) => {
                      const b = e.currentTarget.value;
                      if (b) gitOp("git_checkout", { branch: b });
                    }}
                    onMouseDown={async () => {
                      if (gitBranches.length) return;
                      const r = await invoke<any>("git_branches").catch(() => null);
                      const list = r?.branches ?? r?.data?.branches ?? r?.result?.branches ?? [];
                      setGitBranches(
                        list.map((b: any) => (typeof b === "string" ? b : (b?.name ?? ""))).filter(Boolean),
                      );
                    }}
                  >
                    <option value="">{t.gitSwitchBranch}</option>
                    {gitBranches.map((b) => (
                      <option key={b} value={b}>
                        {b}
                      </option>
                    ))}
                  </select>
                </div>

                {(gitInfo.staged?.length ?? 0) === 0 && (gitInfo.unstaged?.length ?? 0) === 0 && (
                  <div className="sidebar-empty">{t.gitClean}</div>
                )}

                {(gitInfo.staged?.length ?? 0) > 0 && (
                  <div className="git-group">
                    <div className="git-group-head">
                      <span>{t.gitStaged(gitInfo.staged.length)}</span>
                      <button className="git-mini" onClick={() => gitOp("git_unstage", { paths: null })}>
                        {t.gitUnstageAll}
                      </button>
                    </div>
                    {gitInfo.staged.map((f: any) => (
                      <div key={f.path} className="git-row">
                        <span className="git-type">{changeLetter(f.type)}</span>
                        <span className="git-path">{f.path}</span>
                        <span className="git-stat">
                          <span className="add">+{f.additions ?? 0}</span>{" "}
                          <span className="del">-{f.deletions ?? 0}</span>
                        </span>
                        <button
                          className="git-mini"
                          title={t.gitUnstage}
                          onClick={() => gitOp("git_unstage", { paths: [f.path] })}
                        >
                          −
                        </button>
                      </div>
                    ))}
                  </div>
                )}

                {(gitInfo.unstaged?.length ?? 0) > 0 && (
                  <div className="git-group">
                    <div className="git-group-head">
                      <span>{t.gitChanges(gitInfo.unstaged.length)}</span>
                      <button className="git-mini" onClick={() => gitOp("git_stage", { paths: null })}>
                        {t.gitStageAll}
                      </button>
                    </div>
                    {gitInfo.unstaged.map((f: any) => (
                      <div key={f.path} className="git-row">
                        <span className="git-type">{changeLetter(f.type)}</span>
                        <span className="git-path">{f.path}</span>
                        <span className="git-stat">
                          <span className="add">+{f.additions ?? 0}</span>{" "}
                          <span className="del">-{f.deletions ?? 0}</span>
                        </span>
                        <button
                          className="git-mini"
                          title={t.gitStage}
                          onClick={() => gitOp("git_stage", { paths: [f.path] })}
                        >
                          +
                        </button>
                        <button
                          className="git-mini danger"
                          title={t.gitDiscard}
                          onClick={() => {
                            if (window.confirm(t.gitDiscardConfirm(f.path)))
                              gitOp("git_discard", { paths: [f.path] });
                          }}
                        >
                          ↺
                        </button>
                      </div>
                    ))}
                  </div>
                )}

                <input
                  className="git-msg"
                  placeholder={t.gitMsgPlaceholder}
                  value={commitMsg}
                  onChange={(e) => setCommitMsg(e.currentTarget.value)}
                />
                <div className="git-actions">
                  <button
                    disabled={!commitMsg.trim() || (gitInfo.staged?.length ?? 0) === 0}
                    onClick={async () => {
                      await gitOp("git_commit", { message: commitMsg.trim() });
                      setCommitMsg("");
                    }}
                  >
                    {t.gitDoCommit}
                  </button>
                  <button
                    className="ghost"
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

      {showTasks && (
        <div className="modal-mask" onClick={() => setShowTasks(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title panel-title">
              <IconTerminal size={16} /> {t.tasksTitle}
            </div>

            {bgTasks.length === 0 &&
              subagents.length === 0 &&
              Object.keys(schedTasks).length === 0 && (
                <div className="sidebar-empty">{t.tasksEmpty}</div>
              )}

            {bgTasks.length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksBg(bgTasks.length)}</span>
                </div>
                {bgTasks.map((b) => {
                  // TaskSnapshot 是 snake_case
                  const id = b.task_id ?? b.taskId;
                  const cmd = b.display_command ?? b.command ?? "";
                  const done = b.completed === true;
                  return (
                    <div key={id} className="task-row">
                      <span className={`task-dot ${done ? "done" : "run"}`} />
                      <span className="task-cmd" title={cmd}>
                        {cmd}
                      </span>
                      {done ? (
                        <span className="task-exit">
                          {b.exit_code === 0 ? "exit 0" : `exit ${b.exit_code ?? "?"}`}
                        </span>
                      ) : (
                        <button
                          className="git-mini danger"
                          onClick={async () => {
                            await invoke("task_kill", { taskId: id }).catch((e) =>
                              setError(String(e)),
                            );
                            refreshTasks();
                          }}
                        >
                          {t.taskKill}
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            {subagents.length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksSub(subagents.length)}</span>
                </div>
                {subagents.map((sa) => (
                  // SubagentLiveSnapshotDto 是 camelCase
                  <div key={sa.subagentId} className="task-row">
                    <span className="task-dot run" />
                    <span className="task-cmd" title={sa.description}>
                      <b>{sa.subagentType}</b> {sa.description}
                    </span>
                    <span className="task-exit">
                      {Math.round((sa.durationMs ?? 0) / 1000)}s · {sa.turnCount ?? 0}
                      {t.tasksTurns} · {sa.contextUsagePct ?? 0}%
                    </span>
                    <button
                      className="git-mini danger"
                      onClick={async () => {
                        await invoke("subagent_cancel", { subagentId: sa.subagentId }).catch((e) =>
                          setError(String(e)),
                        );
                        refreshTasks();
                      }}
                    >
                      {t.taskCancel}
                    </button>
                  </div>
                ))}
              </div>
            )}

            {Object.keys(schedTasks).length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksSched(Object.keys(schedTasks).length)}</span>
                </div>
                {Object.values(schedTasks).map((s) => (
                  <div key={s.taskId} className="task-row">
                    <span className="task-dot run" />
                    <span className="task-cmd" title={s.prompt}>
                      <b>{s.humanSchedule}</b> {s.prompt}
                    </span>
                    {s.nextFireAt && (
                      <span className="task-exit" title={s.nextFireAt}>
                        {t.tasksNextFire} {new Date(s.nextFireAt).toLocaleTimeString()}
                      </span>
                    )}
                    <button
                      className="git-mini danger"
                      onClick={() =>
                        // 删除成功后引擎会发 scheduled_task_deleted，由它移除该行；
                        // 这里不做乐观删除，免得和通知重复处理。
                        invoke("scheduler_delete", { taskId: s.taskId }).catch((e) =>
                          setError(String(e)),
                        )
                      }
                    >
                      {t.taskCancel}
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div className="modal-footer">
              <span />
              <button onClick={() => setShowTasks(false)}>{t.close}</button>
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
                  {trustReq.configKinds.map((k) => (
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
              {question.questions.map((q) => (
                <div key={q.question} className="question-block">
                  <div className="question-text">{q.question}</div>
                  {q.multiSelect && <div className="question-multi">{t.questionMulti}</div>}
                  <div className="question-options">
                    {(q.options ?? []).map((o) => {
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
                refreshMcpLive();
                setSettingsTab("mcp");
                setShowSettings(true);
              }}
            >
              <IconGitBranch size={15} /> {t.navMcp}
              {/* 有实时数据时数"真正启用的"，而不是配置文件里的条目数 ——
                  被文件夹信任挡住的服务器不该被算成可用。 */}
              {(mcpLive.length ? mcpLive.filter((s) => s.session?.enabled !== false).length : mcpServers.length) > 0 && (
                <span className="side-nav-count">
                  {mcpLive.length
                    ? mcpLive.filter((s) => s.session?.enabled !== false).length
                    : mcpServers.length}
                </span>
              )}
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
            <>
              {/* 项目内容搜索 —— 引擎侧 ripgrep 语义，尊重 .gitignore */}
              <input
                className="session-search"
                value={grepQuery}
                placeholder={t.grepPlaceholder}
                onChange={(e) => setGrepQuery(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") runGrep();
                  if (e.key === "Escape") {
                    setGrepQuery("");
                    setGrepHits(null);
                  }
                }}
              />
              <div className="session-list tree-list">
                {grepping && <div className="sidebar-empty">{t.searching}</div>}
                {!grepping && grepHits !== null && grepHits.length === 0 && (
                  <div className="sidebar-empty">{t.grepNoHits}</div>
                )}
                {!grepping &&
                  grepHits !== null &&
                  grepHits.map((f) => (
                    <div key={f.path} className="grep-file">
                      <div
                        className="grep-file-head"
                        title={f.path}
                        onClick={() =>
                          setInput((v) => v + (v && !v.endsWith(" ") ? " " : "") + "@" + f.path + " ")
                        }
                      >
                        <IconFile size={12} /> {f.name}
                        <span className="grep-count">{f.matches.length}</span>
                      </div>
                      {f.matches.slice(0, 5).map((m, i) => (
                        <div key={i} className="grep-line" title={m.content}>
                          <span className="grep-lineno">{m.line}</span>
                          <span className="grep-text">{m.content.trim().slice(0, 120)}</span>
                        </div>
                      ))}
                    </div>
                  ))}
                {grepHits === null &&
                  (fileList.length === 0 ? (
                    <div className="sidebar-empty">{sessionId ? "—" : t.emptySubStart}</div>
                  ) : (
                    <TreeView
                      node={buildTree(fileList)}
                      onPick={(p) =>
                        setInput((v) => v + (v && !v.endsWith(" ") ? " " : "") + "@" + p + " ")
                      }
                    />
                  ))}
              </div>
            </>
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
          {/* 底部工作区行 —— 点击可跨项目切换（对应 Claude Code 的跨项目 Recents） */}
          <div className="side-foot">
            {wsMenu && (
              <>
                <div className="plus-backdrop" onClick={() => setWsMenu(false)} />
                <div className="ws-menu">
                  <div className="mode-menu-head">{t.wsSwitch}</div>
                  {knownWorkspaces
                    .filter((w) => w.path !== workspace)
                    .slice(0, 8)
                    .map((w) => (
                      <button
                        key={w.path}
                        className="ws-menu-item"
                        title={w.path}
                        onClick={() => {
                          setWsMenu(false);
                          setWorkspace(w.path);
                          localStorage.setItem("wancode-workspace", w.path);
                          refreshSessions(w.path);
                          startSession(undefined, w.path);
                        }}
                      >
                        <IconFolderClosed size={14} />
                        <span className="ws-menu-name">
                          {w.path.split(/[\\/]/).filter(Boolean).pop()}
                        </span>
                        <span className="ws-menu-count">{w.sessions}</span>
                      </button>
                    ))}
                  <button className="ws-menu-item" onClick={pickFolderAndConnect}>
                    <IconFolder size={14} /> <span className="ws-menu-name">{t.wsBrowse}</span>
                  </button>
                </div>
              </>
            )}
            <button
              className="side-foot-ws"
              onClick={() => {
                if (!workspace) {
                  pickFolderAndConnect();
                  return;
                }
                refreshWorkspaces();
                setWsMenu((v) => !v);
              }}
              disabled={starting}
              title={workspace || t.sugOpenFolder}
            >
              <IconFolder size={14} />
              <span className="side-foot-name">
                {workspace ? workspace.split(/[\\/]/).filter(Boolean).pop() : t.sugOpenFolder}
              </span>
              {workspace && <IconChevron size={12} />}
            </button>
            <div className="side-foot-meta">
              {/* gitInfo === null 表示"还没取到/取失败"，不能当成"不是仓库" */}
              {gitInfo?.isRepo ? (
                <>
                  <IconGitBranch size={11} /> {gitInfo.branch}
                  {(gitInfo.files?.length ?? 0) > 0 && ` · ${t.homeChanged(gitInfo.files.length)}`}
                </>
              ) : gitInfo?.isRepo === false ? (
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

          {/* 跨工作区的近期会话。刻意排除当前工作区——那部分左栏已经列了，
              首页再列一遍就是和左栏打架（之前踩过）。这里只回答左栏答不了的
              问题：我在别的项目干到哪了。 */}
          {otherRecent.length > 0 && (
            <div className="home-recent">
              <div className="home-recent-head">{t.homeOtherProjects}</div>
              {otherRecent.map((s) => (
                <button
                  key={s.sessionId}
                  className="home-recent-row"
                  title={s.path}
                  onClick={() => startSession(s.sessionId, s.path)}
                >
                  <span className="home-recent-proj">{baseName(s.path)}</span>
                  <span className="home-recent-title">
                    {s.title || t.untitledSession}
                  </span>
                  {s.branch && (
                    <span className="home-recent-branch">
                      <IconGitBranch size={11} /> {s.branch}
                    </span>
                  )}
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
          if (it.kind === "thought")
            return (
              <details
                key={i}
                className="msg thought"
                open={openThoughts.has(i)}
                onToggle={(e) => {
                  const isOpen = (e.currentTarget as HTMLDetailsElement).open;
                  setOpenThoughts((prev) => {
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
        {/* 排队中的提示词：Agent 忙时输入不再被拦，引擎按 FIFO 依次执行 */}
        {queue.length > 0 && (
          <div className="queue-strip">
            <div className="queue-head">
              <span className="queue-title">{t.queueTitle(queue.length)}</span>
              <button
                className="queue-clear"
                onClick={() => invoke("agent_queue_clear").catch((e) => setError(String(e)))}
              >
                {t.queueClear}
              </button>
            </div>
            {queue.map((q, n) => (
              <div key={q.id} className="queue-row">
                <span className="queue-idx">{n + 1}</span>
                <span className="queue-text">{q.text}</span>
                <button
                  className="icon-btn queue-x"
                  title={t.queueRemove}
                  onClick={() =>
                    invoke("agent_queue_remove", { id: q.id, expectedVersion: q.version }).catch(
                      (e) => setError(String(e)),
                    )
                  }
                >
                  <IconX size={13} />
                </button>
              </div>
            ))}
          </div>
        )}
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
              // ↑/↓ 调取历史输入：只在没有候选弹窗、且不是在多行文本里移动光标时接管。
              if (e.key === "ArrowUp" && !popup && historyRef.current.length > 0) {
                const atStart = e.currentTarget.selectionStart === 0;
                if (input === "" || histIdxRef.current >= 0 || atStart) {
                  e.preventDefault();
                  if (histIdxRef.current < 0) draftRef.current = input; // 存草稿
                  const next = Math.min(histIdxRef.current + 1, historyRef.current.length - 1);
                  histIdxRef.current = next;
                  onComposerChange(historyRef.current[next] ?? "");
                  return;
                }
              }
              if (e.key === "ArrowDown" && !popup && histIdxRef.current >= 0) {
                e.preventDefault();
                const next = histIdxRef.current - 1;
                histIdxRef.current = next;
                onComposerChange(next < 0 ? draftRef.current : historyRef.current[next]);
                return;
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                histIdxRef.current = -1;
                send();
              }
            }}
            placeholder={
              busy
                ? t.queueHint
                : sessionId
                  ? t.composerPlaceholder
                  : starting
                    ? t.starting
                    : t.composerHint
            }
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
