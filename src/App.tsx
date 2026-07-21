import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { OnboardingWizard } from "./features/onboarding/OnboardingWizard";
import { SettingsModal } from "./features/settings/SettingsModal";
import { GitPanel } from "./features/git/GitPanel";
import { Composer } from "./features/composer/Composer";
import { Messages } from "./features/messages/Messages";
import { Dialogs } from "./features/dialogs/Dialogs";
import { Home } from "./features/home/Home";
import { Workbench } from "./features/workbench/Workbench";
import { CommandPalette } from "./features/palette/CommandPalette";
import { TasksPanel } from "./features/tasks/TasksPanel";
import { TerminalPanel } from "./features/terminal/TerminalPanel";
import { Sidebar } from "./features/sidebar/Sidebar";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { check as checkUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { STRINGS, loadLang, type Lang } from "./i18n";
import {
  IconSettings, IconSun, IconMoon, IconRewind, IconGitBranch,
  IconTerminal, IconFile, IconFolderClosed, IconColumns,
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
  // 首次运行向导：检测到零模型配置时弹出。step 1 = 配 Key，step 2 = 开工作区
  const [showOnboarding, setShowOnboarding] = useState(false);
  // 上次异常退出的会话（崩溃恢复横幅）
  const [crashInfo, setCrashInfo] = useState<{ sessionId: string; workspace: string } | null>(null);
  const [quickPreset, setQuickPreset] = useState("");
  const [quickKey, setQuickKey] = useState("");
  const [quickBusy, setQuickBusy] = useState(false);
  const [quickResult, setQuickResult] = useState("");
  const [modelTestMsg, setModelTestMsg] = useState("");


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
  // v0.14 工作台（右侧第二栏）：Diff 一级视图
  const [showWorkbench, setShowWorkbench] = useState(false);
  const [wbFiles, setWbFiles] = useState<any[] | null>(null);
  const [wbLoading, setWbLoading] = useState(false);
  const [wbOpenPaths, setWbOpenPaths] = useState<Set<string>>(new Set());
  // v0.14 命令面板（Ctrl+K）。动作每次渲染重组进 ref，全局键监听只挂一次。
  const [showPalette, setShowPalette] = useState(false);
  const paletteRef = useRef<{ toggleWorkbench: () => void } | null>(null);
  const [termTab, setTermTab] = useState<"output" | "shell">("output");
  const [ptyOpened, setPtyOpened] = useState(false);
  const [worktrees, setWorktrees] = useState<{ path: string; branch: string }[]>([]);
  const [wtBusy, setWtBusy] = useState(false);
  const [wtMsg, setWtMsg] = useState<{ bad: boolean; text: string } | null>(null);
  const [editingQueueId, setEditingQueueId] = useState<string | null>(null);
  const sentInterjectionsRef = useRef<Set<string>>(new Set());
  // 模糊文件搜索：open 拿 searchId，结果经 x.ai/search/fuzzy/status 通知异步到达
  const fuzzyRef = useRef<{ id: string | null; opening: boolean }>({ id: null, opening: false });
  const [fuzzyHits, setFuzzyHits] = useState<{ path: string; isDir: boolean }[] | null>(null);
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
      // 零模型 = 新用户：只弹向导，不启动引擎——引擎在没有任何模型时
      // 会在启动路径 panic（实测 capacity overflow / RefCell 双崩），
      // 所以「配好模型再启引擎」不只是体验，是活下来的前提。
      try {
        const models = await invoke<any[]>("model_list");
        if (!models || models.length === 0) {
          setShowOnboarding(true);
          return;
        }
      } catch {
        /* model_list 失败不拦启动 */
      }
      // 崩溃恢复：上次异常退出且有会话 → 出横幅，让用户选择恢复或忽略。
      // 不自动恢复——上次崩溃可能正是那个会话引起的。
      try {
        const c = await invoke<any>("crash_recovery_info");
        if (c?.sessionId) {
          setCrashInfo({ sessionId: c.sessionId, workspace: c.workspace ?? "" });
        }
      } catch {
        /* 标记读取失败不影响启动 */
      }
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
  const [skills, setSkills] = useState<
    { name: string; description: string; path: string; enabled?: boolean; scope?: string }[]
  >([]);
  const [skillForm, setSkillForm] = useState({ name: "", description: "" });
  const [editingSkill, setEditingSkill] = useState<{ name: string; path: string; content: string } | null>(null);
  const [migrateMsg, setMigrateMsg] = useState("");

  async function openSkillEditor(name: string, path: string) {
    try {
      const content = await invoke<string>("skill_read", { path });
      setEditingSkill({ name, path, content });
    } catch (e) {
      setError(String(e));
    }
  }

  /// 引擎版技能列表。响应外层 camelCase、内层 SkillInfo 是 snake_case。
  async function refreshSkills() {
    if (!workspaceRefSafe()) return;
    try {
      const r = await invoke<any>("skills_list", { workspace: workspaceRefSafe() });
      setSkills(
        (r?.skills ?? []).map((sk: any) => ({
          name: sk.name,
          description: sk.short_description ?? sk.description ?? "",
          path: sk.path,
          enabled: sk.enabled !== false,
          scope: typeof sk.scope === "string" ? sk.scope : "",
        })),
      );
    } catch (e) {
      setError(`skills: ${String(e)}`);
    }
  }
  function workspaceRefSafe() {
    return workspace;
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
    // bypass/auto 此前只是客户端自动应答——引擎照样发权限请求、走一个来回。
    // 同步给引擎后它直接跳过请求。失败不回滚 UI：客户端自动应答仍兜底。
    if (sessionId) {
      invoke("agent_sync_permission_mode", {
        yolo: next === "bypass",
        auto: next === "auto",
      }).catch(() => {});
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
      refreshSkills();
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

  // Git 面板一打开就拉 worktree 列表。挂在面板可见状态而不是某个入口按钮
  // 上——之前设置页就是因为把数据加载挂在入口按钮，换个路径进去就是空的。
  useEffect(() => {
    if (showGit) { setWtMsg(null); refreshWorktrees(); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showGit, sessionId]);

  // 首页的“别的项目干到哪了”。当前工作区排除掉——左栏已经列了它的会话。
  async function refreshOtherRecent(ws: string) {
    if (!sessionIdRef.current) return;
    try {
      // 引擎的 recent 列表（session_summaries/workspace_list_recent，raw 响应
      // 是 Summary 数组）；失败退回本地聚合的 recent_sessions。
      let all: any[];
      try {
        const r = await invoke<any>("workspace_list_recent", { limit: 30 });
        all = (Array.isArray(r) ? r : []).map((s: any) => ({
          path: s.info?.cwd ?? "",
          sessionId: s.info?.id ?? "",
          title: s.session_summary ?? "",
          updatedAt: s.updated_at ?? "",
          branch: s.head_branch ?? "",
          messages: s.num_chat_messages ?? 0,
        })).filter((x: any) => x.messages > 0);
      } catch {
        all = await invoke<any[]>("recent_sessions", { limit: 30 });
      }
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

  /// 工作台 Diff 数据：引擎 x.ai/git/diffs（includePatch）。
  /// 信封语义同 refreshGit：先判 error，result 为 null 视为非仓库。
  async function refreshWorkbench() {
    if (!sessionIdRef.current) {
      setWbFiles(null);
      return;
    }
    setWbLoading(true);
    try {
      const r = await invoke<any>("git_diffs", { paths: null, includePatch: true });
      if (r?.error) {
        setWbFiles(null);
        setError(`diff: ${typeof r.error === "string" ? r.error : JSON.stringify(r.error)}`);
        return;
      }
      const env = r?.result ?? r;
      const d = env?.data ?? env;
      setWbFiles(Array.isArray(d?.files) ? d.files : null);
    } catch (e) {
      setWbFiles(null);
      setError(String(e));
    } finally {
      setWbLoading(false);
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
    refreshSkills();
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
          // 恢复会话重播：插话在 chat_history 里是包了
          // "The user sent a message while you were working:\n<user_query>…</user_query>"
          // 的合成消息。解包还原成 ⚡ 样式，而不是把包装文本原样糊出来。
          const ijm = /^The user sent a message while you were working:\s*\n<user_query>\n?([\s\S]*?)\n?<\/user_query>\s*$/.exec(
            u.content.text,
          );
          if (ijm) {
            appendStream("user", `⚡ ${ijm[1]}`);
          } else if (u.content.text === lastSentRef.current) {
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
        // 模糊搜索结果流（generation 去重引擎侧已做，直接覆盖显示）
        if (m === "x.ai/search/fuzzy/status") {
          const p = e.payload?.params ?? {};
          if (p.searchId === fuzzyRef.current.id && Array.isArray(p.matches)) {
            setFuzzyHits(p.matches.map((x: any) => ({ path: x.path, isDir: !!x.is_dir })));
          }
          return;
        }
        // P2.10 通知监听：能力对应的刷新函数已存在，直接挂接
        if (m === "x.ai/git_head_changed" || m === "x.ai/gitHeadChanged") {
          refreshGit();
          return;
        }
        if (m === "x.ai/sessions/changed") {
          if (workspace) refreshSessions(workspace);
          return;
        }
        if (
          m === "x.ai/mcp_initialized" ||
          m === "x.ai/mcp/init_progress" ||
          m === "x.ai/mcp/server_status"
        ) {
          refreshMcpLive();
          return;
        }
        if (m === "x.ai/config_changed") {
          // 配置热变更（外部编辑 config.toml）
          refreshMcpLive();
          return;
        }
        // 插话回显：引擎向所有窗格广播；自己发的（id 在集合里）跳过，
        // 其他客户端发起的照常渲染
        if (m === "x.ai/session/interjection") {
          const p = e.payload?.params ?? {};
          // 广播里的键是 interjectionId（不是 id）——读错键 = 去重永不命中，
          // 自己的插话会显示两遍。实测踩过。
          const iid = p.interjectionId;
          if (iid && sentInterjectionsRef.current.has(iid)) {
            sentInterjectionsRef.current.delete(iid);
          } else if (p.text) {
            setItems((prev) => [...prev, { kind: "user", text: `⚡ ${p.text}` }]);
          }
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
    // 换会话先清面板数据——失败时留着上一个工作区的 git 状态最危险（#83）
    setGitInfo(null);
    try {
      const r = await invoke<{ session_id: string; models: string[]; cwd: string }>(
        "agent_start",
        {
          workspace: wsPath,
          model,
          resume: resume ?? null,
        },
      );
      setSessionId(r.session_id);
      sessionIdRef.current = r.session_id;
      // 工作区标签以会话真实 cwd 为准（#83：标签与会话脱节时，git 面板
      // 显示的是另一个仓库的改动，stash/丢弃会打错目标）。
      if (r.cwd) {
        setWorkspace(r.cwd);
        localStorage.setItem("wancode-workspace", r.cwd);
      }
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
      const msg = String(e);
      // 后端启动不变量的结构化错误（v0.12.2 契约）：
      // MODEL_REQUIRED = 零模型 → 这不是"报错"，是"该走向导了"。
      if (msg.includes("MODEL_REQUIRED")) {
        setShowOnboarding(true);
      } else {
        setError(msg);
      }
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

  // ── worktree 并行 Agent ────────────────────────────────────────
  async function refreshWorktrees() {
    if (!sessionIdRef.current) return;
    try {
      const r = await invoke<any>("worktree_list");
      const arr = Array.isArray(r) ? r : (r?.worktrees ?? r?.data ?? []);
      setWorktrees(
        (Array.isArray(arr) ? arr : [])
          .map((w: any) => ({
            path: w.path ?? w.worktree_path ?? w.worktreePath ?? "",
            branch: w.branch ?? w.head_branch ?? "",
          }))
          .filter((w: any) => w.path),
      );
    } catch (e) {
      setError(`worktree: ${String(e)}`);
    }
  }

  /// 把当前会话搬进一个新 worktree 并切过去。
  /// 引擎返回的是 **fork 出来的新 session id**，不是传进去的那个。
  async function forkIntoWorktree() {
    if (!workspace || wtBusy) return;
    setWtBusy(true);
    setError("");
    try {
      const r = await invoke<any>("worktree_resume_session", { workspace });
      const newId = r?.sessionId;
      const cwd = r?.effectiveCwd || r?.worktreePath;
      if (!newId || !cwd) throw new Error(`返回缺字段: ${JSON.stringify(r)}`);
      setShowGit(false);
      setWorkspace(cwd);
      await startSession(newId, cwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setWtBusy(false);
    }
  }

  /// 合回主目录。响应是 status 分派的：conflicts 分支必须如实报出来，
  /// 当成成功处理的话用户会以为改动已经合上了。
  async function applyWorktree(path: string) {
    setWtBusy(true);
    setWtMsg(null);
    try {
      const r = await invoke<any>("worktree_apply", { worktreePath: path });
      if (r?.status === "conflicts") {
        const files = (r.conflicts ?? []).map((c: any) => c.path ?? "?").join(", ");
        // 就地显示：这条曾经写进 setError，而错误条渲染在聊天区、被 Git 弹窗
        // 盖住——用户点完“合回”什么都看不到，冲突就成了静默无事发生。
        setWtMsg({ bad: true, text: t.wtConflicts(files || String((r.conflicts ?? []).length)) });
      } else {
        const n = (r?.files ?? []).length;
        setWtMsg({ bad: false, text: t.wtApplied(n) });
        setItems((prev) => [...prev, { kind: "note", text: t.wtApplied(n) }]);
        refreshGit();
      }
    } catch (e) {
      setWtMsg({ bad: true, text: String(e) });
    } finally {
      setWtBusy(false);
    }
  }

  async function removeWorktree(path: string) {
    setWtBusy(true);
    try {
      await invoke("worktree_remove", { idOrPath: path, force: true });
      await refreshWorktrees();
    } catch (e) {
      setError(String(e));
    } finally {
      setWtBusy(false);
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

  /// Mid-turn steering: inject the composer text into the RUNNING turn
  /// without cancelling it and without queueing. The engine broadcasts
  /// `x.ai/session/interjection` to all panes — we mint the id so the
  /// listener can skip our own echo (we render optimistically here).
  async function sendInterject() {
    const text = input.trim();
    if (!text || !sessionIdRef.current) return;
    const id = crypto.randomUUID();
    sentInterjectionsRef.current.add(id);
    setInput("");
    onComposerChange("");
    setItems((prev) => [...prev, { kind: "user", text: `⚡ ${text}` }]);
    try {
      await invoke("agent_interject", { text, interjectionId: id });
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
      ? (fuzzyHits ??
          fileList
            .filter((f) => f.toLowerCase().includes(popup.query.toLowerCase()))
            .map((f) => ({ path: f, isDir: false })))
          .slice(0, 8)
          .map((f) => {
            // 引擎返回的是绝对路径；@ 引用要的是工作区相对路径
            let rel = f.path.replace(/\\/g, "/");
            const ws = workspace.replace(/\\/g, "/").replace(/\/+$/, "");
            if (ws && rel.toLowerCase().startsWith(ws.toLowerCase() + "/"))
              rel = rel.slice(ws.length + 1);
            return { label: rel + (f.isDir ? "/" : "") };
          })
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
      fuzzyKick(m[1]);
    } else {
      if (popup?.kind === "at") fuzzyStop();
      setPopup(null);
    }
  }

  /// 引擎模糊搜索：首次打开拿 searchId，之后 change 只 ack、结果走通知。
  async function fuzzyKick(q: string) {
    if (!sessionIdRef.current) return;
    const f = fuzzyRef.current;
    try {
      if (!f.id && !f.opening) {
        f.opening = true;
        const r = await invoke<any>("fuzzy_open", { workspace });
        f.id = r?.searchId ?? null;
        f.opening = false;
      }
      if (f.id) await invoke("fuzzy_change", { searchId: f.id, query: q, limit: 8 });
    } catch {
      f.opening = false;
      // 失败退回本地 fileList 过滤，不打扰
    }
  }

  function fuzzyStop() {
    const id = fuzzyRef.current.id;
    fuzzyRef.current = { id: null, opening: false };
    setFuzzyHits(null);
    if (id) invoke("fuzzy_close", { searchId: id }).catch(() => {});
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

  const toggleWorkbench = () => {
    const next = !showWorkbench;
    setShowWorkbench(next);
    if (next) refreshWorkbench();
  };
  paletteRef.current = { toggleWorkbench };
  // 全局快捷键：Ctrl+K 面板、Ctrl+Shift+D 工作台、Ctrl+` 终端。
  // 只挂一次监听，经 ref 取当前闭包，避免每渲染重挂。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setShowPalette((v) => !v);
      } else if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "d") {
        e.preventDefault();
        paletteRef.current?.toggleWorkbench();
      } else if (e.ctrlKey && e.key === "`") {
        e.preventDefault();
        setShowTerminal((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const paletteActions = [
    {
      id: "new-session",
      label: t.sidebarNewSession,
      run: () => {
        setSessionId("");
        sessionIdRef.current = "";
        setItems([]);
        setSidebarTab("sessions");
      },
    },
    { id: "open-folder", label: t.sugOpenFolder, run: pickFolderAndConnect },
    { id: "workbench", label: t.wbTooltip, hint: "Ctrl+Shift+D", disabled: !sessionId, run: toggleWorkbench },
    {
      id: "terminal",
      label: t.paletteTerminal,
      hint: "Ctrl+`",
      disabled: !sessionId,
      run: () => setShowTerminal((v) => !v),
    },
    {
      id: "git",
      label: t.git,
      disabled: !sessionId,
      run: () => {
        refreshGit();
        setShowGit(true);
      },
    },
    { id: "rewind", label: t.rewindTooltip, disabled: !sessionId, run: openRewind },
    {
      id: "tasks",
      label: t.tasksTitle,
      disabled: !sessionId,
      run: () => {
        refreshTasks();
        setShowTasks(true);
      },
    },
    { id: "compact", label: t.compactTitle, disabled: !sessionId || busy, run: runCompact },
    { id: "settings", label: t.settings, run: () => setShowSettings(true) },
    {
      id: "settings-models",
      label: t.paletteModels,
      run: () => {
        setSettingsTab("models");
        setShowSettings(true);
      },
    },
    {
      id: "settings-mcp",
      label: t.paletteMcp,
      run: () => {
        refreshMcpConfig();
        setSettingsTab("mcp");
        setShowSettings(true);
      },
    },
    { id: "theme", label: t.paletteTheme, run: () => setTheme((th) => (th === "dark" ? "light" : "dark")) },
  ];

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
            className={`icon-btn ${showWorkbench ? "active" : ""}`}
            title={t.wbTooltip}
            onClick={() => {
              const next = !showWorkbench;
              setShowWorkbench(next);
              if (next) refreshWorkbench();
            }}
          >
            <IconColumns size={15} />
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

      <Dialogs {...{ answers, doRewind, editingSkill, planApproval, planFeedback, question, refreshSkills, respondPlan, respondQuestion, rewindMode, rewindPoints, setEditingSkill, setError, setPlanFeedback, setRewindMode, setRewindPoints, setTrustReq, toggleAnswer, trustReq, t }} />

      {crashInfo && (
        <div className="crash-banner">
          <span>{t.crashPrompt}</span>
          <button
            onClick={async () => {
              const c = crashInfo;
              setCrashInfo(null);
              await invoke("crash_recovery_ack").catch(() => {});
              if (c) startSession(c.sessionId, c.workspace || undefined);
            }}
          >
            {t.crashRestore}
          </button>
          <button
            className="ghost"
            onClick={() => {
              setCrashInfo(null);
              invoke("crash_recovery_ack").catch(() => {});
            }}
          >
            {t.crashDismiss}
          </button>
        </div>
      )}

      {/* 首次运行向导：贴 Key → 开工作区，两步进入可用状态 */}
      {showOnboarding && (
        <OnboardingWizard
          t={t}
          onConfigured={refreshModels}
          onOpenFolder={() => {
            setShowOnboarding(false);
            pickFolderAndConnect();
          }}
          onCustomEndpoint={() => {
            setShowOnboarding(false);
            setShowSettings(true);
            setSettingsTab("models");
          }}
          onClose={() => setShowOnboarding(false)}
        />
      )}

      <SettingsModal {...{ showSettings, hookForm, lang, mcpForm, mcpList, mcpLive, migrateMsg, modelForm, modelList, modelTestMsg, openSkillEditor, quickBusy, quickKey, quickPreset, quickResult, refreshMcpConfig, refreshMcpLive, refreshModels, refreshSessions, refreshSkills, runUpdate, saveHooks, saveModel, setError, setHookForm, setLang, setMcpForm, setMigrateMsg, setModelForm, setQuickBusy, setQuickKey, setQuickPreset, setQuickResult, setSettingsTab, setShowSettings, setSkillForm, setSkills, setTheme, settingsTab, skillForm, skills, testModel, theme, updateMsg, version, workspace, hooks, t }} />

      <GitPanel {...{ applyWorktree, changeLetter, commitMsg, forkIntoWorktree, gitBranches, gitInfo, gitOp, refreshGit, removeWorktree, sendText, setCommitMsg, setError, setGitBranches, setItems, setShowGit, showGit, worktrees, wtBusy, wtMsg, t, lang }} />


      <TasksPanel {...{ bgTasks, refreshTasks, schedTasks, setError, setShowTasks, showTasks, subagents, t }} />




      <div className="body-row">
        <Sidebar {...{ sessionIdRef, TreeView, buildTree, fileList, gitInfo, grepHits, grepQuery, grepping, input, knownWorkspaces, mcpLive, mcpServers, pickFolderAndConnect, refreshMcpConfig, refreshMcpLive, refreshSessions, refreshSkills, refreshWorkspaces, runGrep, runSearch, searchHits, searchQuery, sessionId, sessions, setError, setGrepHits, setGrepQuery, setInput, setItems, setSessionId, setSettingsTab, setShowSearch, setShowSettings, setSidebarTab, setWorkspace, setWsMenu, showSearch, sidebarTab, skills, startSession, starting, workspace, wsMenu, t, lang }} />

        <div className="main-col">
      <Home {...{ buildSuggestions, baseName, fileList, gitInfo, items, busy, onComposerChange, otherRecent, planSteps, sessionId, setInput, startSession, taRef, t }} />

      <Messages {...{ DiffView, bottomRef, busy, copiedIdx, copyMessage, error, forkFrom, items, openThoughts, permission, respondPermission, setOpenThoughts, workspace, t }} />

      <TerminalPanel {...{ lang, ptyOpened, sessionId, setError, setPtyOpened, setShowTerminal, setTermTab, setTerminalLines, showTerminal, termTab, terminalLines, theme, t }} />

      <Composer {...{ MODE_ORDER, acceptPopup, busy, draftRef, editingQueueId, fileInputRef, histIdxRef, historyRef, input, lang, model, modeMenu, modeMeta, models, onComposerChange, onPaste, onPickImages, pastedImages, permMode, pickFolderAndConnect, plusMenu, popup, popupItems, queue, refreshMcpConfig, send, sendInterject, sessionId, setEditingQueueId, setError, setInput, setItems, setMode, setModeMenu, setModel, setPastedImages, setPlusMenu, setPopup, setSettingsTab, setShowSettings, setShowTerminal, starting, taRef, workspace, t }} />
        </div>

        <Workbench {...{ showWorkbench, setShowWorkbench, wbFiles, wbLoading, wbOpenPaths, setWbOpenPaths, refreshWorkbench, gitOp, t }} />
      </div>
      {showPalette && <CommandPalette actions={paletteActions} onClose={() => setShowPalette(false)} t={t} />}
    </main>
  );
}

export default App;
