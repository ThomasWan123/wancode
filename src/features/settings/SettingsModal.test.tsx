/* 设置弹窗的特征测试（v0.23.x 设置重排前的安全网）。
   这个组件自 v0.13 拆出后没有任何用例：7 个标签各发哪些 Tauri 命令、
   什么时候刷新列表，全靠人眼记。这里如实记录**当前**行为——重排导航、
   拆分组件时，这些断言就是"没改坏"的依据。 */
import { useState } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { STRINGS } from "../../i18n";
import {
  SettingsModal,
  type SettingsModelEntry,
  type SettingsModalProps,
  type SettingsMcpLiveServer,
  type SettingsSkill,
} from "./SettingsModal";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const t = STRINGS.en;
const WORKSPACE = "D:\\proj";

function makeProps(overrides: Partial<SettingsModalProps> = {}): SettingsModalProps {
  return {
    showSettings: true,
    settingsTab: "general",
    setSettingsTab: vi.fn(),
    setShowSettings: vi.fn(),
    lang: "en",
    setLang: vi.fn(),
    theme: "dark",
    setTheme: vi.fn(),
    hookForm: { event: "PostToolUse", command: "" },
    setHookForm: vi.fn(),
    hooks: [],
    saveHooks: vi.fn(),
    mcpForm: { name: "", command: "", args: "", url: "" },
    mcpList: [],
    mcpLive: [],
    setMcpForm: vi.fn(),
    refreshMcpConfig: vi.fn(),
    refreshMcpLive: vi.fn(),
    refreshSessions: vi.fn(),
    migrateMsg: "",
    setMigrateMsg: vi.fn(),
    modelForm: { key: "", name: "", model: "", base_url: "", api_key: "" },
    modelList: [],
    modelTestMsg: "",
    setModelForm: vi.fn(),
    refreshModels: vi.fn(),
    saveModel: vi.fn(),
    testModel: vi.fn(),
    openSkillEditor: vi.fn(),
    quickPreset: "",
    quickKey: "",
    quickBusy: false,
    quickResult: "",
    setQuickPreset: vi.fn(),
    setQuickKey: vi.fn(),
    setQuickBusy: vi.fn(),
    setQuickResult: vi.fn(),
    skills: [],
    skillForm: { name: "", description: "" },
    setSkills: vi.fn(),
    setSkillForm: vi.fn(),
    refreshSkills: vi.fn(),
    sessionId: "",
    surface: "code",
    workspace: WORKSPACE,
    setError: vi.fn(),
    version: "0.23.7",
    updateMsg: "",
    runUpdate: vi.fn(),
    t,
    ...overrides,
  } satisfies SettingsModalProps;
}

/** 按 App 的方式渲染：表单驱动的 setter 接真实 state，输入与点击才会
    真正流经组件，而不是停在一个 spy 上。其余回调仍是 makeProps 里的 spy。 */
function renderModal(overrides: Partial<SettingsModalProps> = {}) {
  const base = makeProps(overrides);
  function Harness() {
    const [state, setState] = useState<SettingsModalProps>(base);
    const next = (patch: Partial<SettingsModalProps>) => setState((s) => ({ ...s, ...patch }));
    return (
      <SettingsModal
        {...state}
        setSettingsTab={(settingsTab) => next({ settingsTab })}
        setQuickPreset={(quickPreset) => next({ quickPreset })}
        setQuickKey={(quickKey) => next({ quickKey })}
        setQuickBusy={(quickBusy) => next({ quickBusy })}
        setQuickResult={(quickResult) => next({ quickResult })}
        setMcpForm={(mcpForm) => next({ mcpForm })}
        setHookForm={(hookForm) => next({ hookForm })}
        setModelForm={(modelForm) => next({ modelForm })}
        setSkillForm={(skillForm) => next({ skillForm })}
        setSkills={(skills) => next({ skills })}
      />
    );
  }
  render(<Harness />);
  return base;
}

const MODEL: SettingsModelEntry = {
  key: "glm",
  name: "GLM 5.3",
  model: "glm-5.3",
  base_url: "https://open.bigmodel.cn/api/paas/v4",
  env_key: "WANCODE_KEY_GLM",
  has_key: true,
  managed: true,
  caps: {
    caps: {
      text: { state: "supported", source: "config" },
      tool_use: { state: "supported", source: "built_in" },
      vision_input: { state: "unsupported", source: "unknown" },
      reasoning: { state: "unknown", source: "unknown" },
    },
    issues: [],
  },
};

const LIVE: SettingsMcpLiveServer[] = [
  {
    name: "web-search",
    displayName: "Web Search",
    sourceLabel: "config.toml",
    session: {
      enabled: true,
      status: "ready",
      authRequired: true,
      tools: [
        { name: "search", description: "Search the web" },
        { name: "fetch", description: "Fetch a page", enabled: false },
      ],
    },
  },
];

const SKILL: SettingsSkill = { name: "pdf", description: "PDF tools", path: "/skills/pdf/SKILL.md", enabled: true, scope: "user" };

const DIAGNOSTICS = {
  schema_version: 1,
  event_count: 12,
  ledger_sha256: "abcdef1234567890",
  session_ids: ["s1", "s2"],
  open_turns: [],
  duplicate_event_ids: [] as string[],
};

beforeEach(() => {
  // mockReset 之后必须让它重新返回 Promise：真实 invoke 永远返回 Promise，
  // 组件里到处是 invoke(...).catch(...)，返回 undefined 会在测试里凭空炸出
  // 一个生产环境不可能出现的 TypeError。
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
  localStorage.clear();
});

describe("SettingsModal structure", () => {
  it("renders nothing while closed", () => {
    render(<SettingsModal {...makeProps({ showSettings: false })} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens as a named modal dialog", () => {
    render(<SettingsModal {...makeProps()} />);
    expect(screen.getByRole("dialog", { name: "Settings" })).toHaveAttribute("aria-modal", "true");
  });

  it("lists the seven sections in order and marks only the active one", () => {
    render(<SettingsModal {...makeProps()} />);
    const nav = screen.getByRole("dialog").querySelector("nav") as HTMLElement;
    const items = within(nav).getAllByRole("button");
    expect(items.map((item) => item.textContent)).toEqual([
      t.navGeneral,
      t.navModels,
      t.navMcp,
      t.navSkills,
      t.navHooks,
      t.navMemory,
      t.navAbout,
    ]);
    expect(items[0]).toHaveClass("active");
    expect(items.filter((item) => item.classList.contains("active"))).toHaveLength(1);
  });

  it("routes a section click through setSettingsTab", async () => {
    const props = makeProps();
    render(<SettingsModal {...props} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.navMcp }));
    expect(props.setSettingsTab).toHaveBeenCalledWith("mcp");
  });

  it("switches sections and keeps the footer Close action", async () => {
    renderModal();
    await userEvent.setup().click(screen.getByRole("button", { name: t.navAbout }));
    expect(screen.getByText("WanCode v0.23.7")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t.close })).toBeInTheDocument();
  });

  it("closes on Escape, on the mask and on Close, but not on a click inside", async () => {
    const props = makeProps();
    const { container } = render(<SettingsModal {...props} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("dialog"));
    expect(props.setShowSettings).not.toHaveBeenCalled();
    // 三条关闭路径逐条核对传入值：只数次数的话，"Esc 反而打开"也数得过去。
    await user.keyboard("{Escape}");
    expect(props.setShowSettings).toHaveBeenCalledTimes(1);
    expect(props.setShowSettings).toHaveBeenLastCalledWith(false);
    fireEvent.click(container.querySelector(".modal-mask") as HTMLElement);
    expect(props.setShowSettings).toHaveBeenCalledTimes(2);
    expect(props.setShowSettings).toHaveBeenLastCalledWith(false);
    await user.click(screen.getByRole("button", { name: t.close }));
    expect(props.setShowSettings).toHaveBeenCalledTimes(3);
    expect(props.setShowSettings).toHaveBeenLastCalledWith(false);
  });

  it("moves focus into the dialog and keeps Tab inside it", async () => {
    render(<button>Background action</button>);
    render(<SettingsModal {...makeProps()} />);
    const user = userEvent.setup();
    const first = screen.getByRole("button", { name: t.navGeneral });
    const last = screen.getByRole("button", { name: t.close });
    expect(first).toHaveFocus();
    await user.tab({ shift: true });
    expect(last).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
    expect(screen.getByRole("button", { name: "Background action" })).not.toHaveFocus();
  });
});

describe("SettingsModal general", () => {
  it("shows the current language and theme and reports both changes", async () => {
    const props = makeProps();
    render(<SettingsModal {...props} />);
    const [language, theme] = screen.getAllByRole("combobox");
    expect(language).toHaveValue("en");
    expect(theme).toHaveValue("dark");
    const user = userEvent.setup();
    await user.selectOptions(language, "zh");
    expect(props.setLang).toHaveBeenCalledWith("zh");
    expect(localStorage.getItem("wancode-lang")).toBe("zh");
    await user.selectOptions(theme, "light");
    expect(props.setTheme).toHaveBeenCalledWith("light");
  });
});

describe("SettingsModal models", () => {
  it("toggles the selected quick-setup preset", async () => {
    renderModal({ settingsTab: "models", quickPreset: "glm-coding" });
    const coding = screen.getByRole("button", { name: new RegExp(t.presetGlmCoding) });
    const open = screen.getByRole("button", { name: new RegExp(t.presetGlmOpen) });
    expect(coding).toHaveClass("active");
    const user = userEvent.setup();
    await user.click(coding);
    expect(coding).not.toHaveClass("active");
    await user.click(open);
    expect(open).toHaveClass("active");
  });

  it("keeps quick setup disabled until a key is pasted, and disabled while busy", () => {
    const { rerender } = render(
      <SettingsModal {...makeProps({ settingsTab: "models", quickPreset: "deepseek" })} />,
    );
    expect(screen.getByRole("button", { name: t.quickGo })).toBeDisabled();
    rerender(<SettingsModal {...makeProps({ settingsTab: "models", quickPreset: "deepseek", quickKey: "  " })} />);
    expect(screen.getByRole("button", { name: t.quickGo })).toBeDisabled();
    rerender(<SettingsModal {...makeProps({ settingsTab: "models", quickPreset: "deepseek", quickKey: "sk-test" })} />);
    expect(screen.getByRole("button", { name: t.quickGo })).toBeEnabled();
    rerender(
      <SettingsModal {...makeProps({ settingsTab: "models", quickPreset: "deepseek", quickKey: "sk-test", quickBusy: true })} />,
    );
    expect(screen.getByRole("button", { name: t.quickTesting })).toBeDisabled();
  });

  it("quick setup saves through one command, then clears the key and refreshes", async () => {
    vi.mocked(invoke).mockResolvedValue({
      models: [{ name: "DeepSeek Chat" }, { name: "DeepSeek Reasoner" }],
      mcpSeeded: false,
    });
    const base = renderModal({ settingsTab: "models", quickPreset: "deepseek" });
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText(t.quickKeyPlaceholder), "sk-test");
    await user.click(screen.getByRole("button", { name: t.quickGo }));
    await waitFor(() => expect(base.refreshModels).toHaveBeenCalledTimes(1));
    expect(invoke).toHaveBeenCalledWith("provider_quick_setup", { preset: "deepseek", apiKey: "sk-test" });
    expect(screen.getByPlaceholderText(t.quickKeyPlaceholder)).toHaveValue("");
    // 成功提示带 ✅ 前缀，后接预设名；未播种 MCP 时不追加那半句。
    expect(screen.getByText(/DeepSeek Chat、DeepSeek Reasoner/).textContent).toBe(
      `✅ ${t.quickDone}DeepSeek Chat、DeepSeek Reasoner`,
    );
  });

  it("quick setup: a failed connection test refreshes nothing, keeps the key and reports the error", async () => {
    vi.mocked(invoke).mockRejectedValue("connection test failed");
    const base = renderModal({ settingsTab: "models", quickPreset: "glm-coding" });
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText(t.quickKeyPlaceholder), "sk-bad");
    await user.click(screen.getByRole("button", { name: t.quickGo }));
    await screen.findByText("❌ connection test failed");
    expect(base.refreshModels).not.toHaveBeenCalled();
    expect(screen.getByPlaceholderText(t.quickKeyPlaceholder)).toHaveValue("sk-bad");
  });

  it("lists configured models with key status and three-state capability badges", () => {
    // 第二条把三种状态各再出现一次，且与第一条不同——徽章文本全局唯一，
    // 断言才不会落到"任意一个匹配"上。
    const unkeyed: SettingsModelEntry = {
      ...MODEL,
      key: "other",
      name: "Other",
      model: "other-1",
      base_url: "https://other.example/v1",
      has_key: false,
      caps: {
        caps: {
          text: { state: "unknown", source: "unknown" },
          tool_use: { state: "unknown", source: "unknown" },
          vision_input: { state: "supported", source: "config" },
          reasoning: { state: "unsupported", source: "config" },
        },
        issues: [{ catalog_key: "other", field: "tool_use", kind: "inconsistent" }],
      },
    };
    render(<SettingsModal {...makeProps({ settingsTab: "models", modelList: [MODEL, unkeyed] })} />);
    expect(screen.getByText("GLM 5.3")).toBeInTheDocument();
    expect(screen.getByText(`glm-5.3 · ${MODEL.base_url}`)).toBeInTheDocument();
    expect(screen.getByText(t.modelKeyStored)).toBeInTheDocument();
    expect(screen.getByText(t.modelKeyMissing)).toBeInTheDocument();
    for (const badge of ["Tools✓", "Vision✕", "Reasoning?", "Tools?", "Vision✓", "Reasoning✕"]) {
      expect(screen.getByText(badge)).toBeInTheDocument();
    }
    expect(screen.getByText(/tool_use:inconsistent/)).toBeInTheDocument();
  });

  it("shows the empty state when no model is configured", () => {
    render(<SettingsModal {...makeProps({ settingsTab: "models" })} />);
    expect(screen.getByText(t.modelsEmpty)).toBeInTheDocument();
  });

  it("deletes a model through model_remove and refreshes the list", async () => {
    const base = makeProps({ settingsTab: "models", modelList: [MODEL] });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.modelDelete }));
    expect(invoke).toHaveBeenCalledWith("model_remove", { key: "glm" });
    expect(base.refreshModels).toHaveBeenCalledTimes(1);
  });

  it("a quick-fill chip fills the form from its preset", async () => {
    const props = makeProps({ settingsTab: "models" });
    render(<SettingsModal {...props} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "DeepSeek" }));
    expect(props.setModelForm).toHaveBeenCalledWith({
      key: "deepseek",
      name: "DeepSeek V3",
      model: "deepseek-chat",
      base_url: "https://api.deepseek.com/v1",
      api_key: "",
    });
  });

  it("a quick-fill chip keeps an ID and a key the user already typed", async () => {
    const props = makeProps({
      settingsTab: "models",
      modelForm: { key: "mine", name: "", model: "", base_url: "", api_key: "sk-keep" },
    });
    render(<SettingsModal {...props} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Ollama" }));
    expect(props.setModelForm).toHaveBeenCalledWith({
      key: "mine",
      name: "Ollama (Local)",
      model: "qwen2.5-coder",
      base_url: "http://localhost:11434/v1",
      api_key: "sk-keep",
    });
  });

  it("typing in the form reports the patched form", async () => {
    const props = makeProps({ settingsTab: "models" });
    render(<SettingsModal {...props} />);
    await userEvent.setup().type(screen.getByPlaceholderText(t.modelIdField), "g");
    expect(props.setModelForm).toHaveBeenLastCalledWith({
      key: "",
      name: "",
      model: "g",
      base_url: "",
      api_key: "",
    });
  });

  it("routes Save and Test to the supplied handlers", async () => {
    const props = makeProps({
      settingsTab: "models",
      modelForm: { key: "mine", name: "", model: "m", base_url: "https://x", api_key: "" },
    });
    render(<SettingsModal {...props} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: t.modelTest }));
    expect(props.testModel).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: t.modelSave }));
    expect(props.saveModel).toHaveBeenCalledTimes(1);
  });

  it("keeps Test disabled until endpoint and model are both set", () => {
    render(
      <SettingsModal
        {...makeProps({
          settingsTab: "models",
          modelForm: { key: "k", name: "", model: "m", base_url: "", api_key: "" },
        })}
      />,
    );
    expect(screen.getByRole("button", { name: t.modelTest })).toBeDisabled();
  });

  it("migrates plaintext keys, reports the count and refreshes the list", async () => {
    vi.mocked(invoke).mockResolvedValue(2);
    const base = makeProps({ settingsTab: "models" });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.migrateKeys }));
    expect(invoke).toHaveBeenCalledWith("migrate_env_keys");
    expect(base.setMigrateMsg).toHaveBeenCalledWith(t.migrateOk(2));
    expect(base.refreshModels).toHaveBeenCalledTimes(1);
  });

  it("shows the migrate message it is given", () => {
    render(<SettingsModal {...makeProps({ settingsTab: "models", migrateMsg: t.migrateOk(0) })} />);
    expect(screen.getByText(t.migrateOk(0))).toBeInTheDocument();
  });
});

describe("SettingsModal mcp", () => {
  it("live list: a server switch toggles through the engine and refreshes", async () => {
    const base = makeProps({ settingsTab: "mcp", mcpLive: LIVE });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpDisable }));
    expect(invoke).toHaveBeenCalledWith("mcp_toggle", { serverName: "web-search", enabled: false });
    expect(base.refreshMcpLive).toHaveBeenCalledTimes(1);
    expect(screen.getByText("ready")).toHaveClass("mcp-status", "ready");
    expect(screen.getByText("config.toml")).toHaveClass("mcp-src");
  });

  it("live list: a tool click flips that tool only", async () => {
    const base = makeProps({ settingsTab: "mcp", mcpLive: LIVE });
    render(<SettingsModal {...base} />);
    // 已停用的工具带删除线样式，其余不带。
    expect(screen.getByRole("button", { name: "fetch" })).toHaveClass("off");
    expect(screen.getByRole("button", { name: "search" })).not.toHaveClass("off");
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "fetch" }));
    expect(invoke).toHaveBeenCalledWith("mcp_toggle_tool", {
      serverName: "web-search",
      toolName: "fetch",
      enabled: true,
    });
    await user.click(screen.getByRole("button", { name: "search" }));
    expect(invoke).toHaveBeenLastCalledWith("mcp_toggle_tool", {
      serverName: "web-search",
      toolName: "search",
      enabled: false,
    });
    expect(base.refreshMcpLive).toHaveBeenCalledTimes(2);
  });

  it("live list: an auth-required server offers an OAuth trigger that forces a live refresh", async () => {
    const base = makeProps({ settingsTab: "mcp", mcpLive: LIVE });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpAuth }));
    expect(invoke).toHaveBeenCalledWith("mcp_auth_trigger", { serverName: "web-search" });
    expect(base.refreshMcpLive).toHaveBeenCalledWith(true);
  });

  it("live list: a disabled server reads as off and hides its tools", () => {
    render(
      <SettingsModal
        {...makeProps({
          settingsTab: "mcp",
          mcpLive: [{ name: "fs", session: { enabled: false, tools: [{ name: "read" }] } }],
        })}
      />,
    );
    expect(screen.getByRole("button", { name: t.mcpEnable })).not.toHaveClass("on");
    expect(screen.queryByRole("button", { name: "read" })).toBeNull();
  });

  it("live list: shows the empty state when nothing is loaded", () => {
    render(<SettingsModal {...makeProps({ settingsTab: "mcp" })} />);
    expect(screen.getByText(t.mcpLiveEmpty)).toBeInTheDocument();
  });

  it("config list: deleting a server refreshes the config and the workspace session list", async () => {
    const base = makeProps({
      settingsTab: "mcp",
      mcpList: [{ name: "fs", command: "npx", args: ["-y", "srv"], enabled: true }],
    });
    render(<SettingsModal {...base} />);
    expect(screen.getByText("npx -y srv")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpDelete }));
    expect(invoke).toHaveBeenCalledWith("mcp_config_remove", { name: "fs" });
    expect(base.refreshMcpConfig).toHaveBeenCalledTimes(1);
    expect(base.refreshSessions).toHaveBeenCalledWith(WORKSPACE);
  });

  it("adds a stdio server with whitespace-split args, then clears the form", async () => {
    const base = renderModal({
      settingsTab: "mcp",
      mcpForm: { name: "fs", command: "npx", args: "-y @modelcontextprotocol/server-filesystem D:\\proj", url: "" },
    });
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpAdd }));
    expect(invoke).toHaveBeenCalledWith("mcp_config_upsert", {
      name: "fs",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-filesystem", "D:\\proj"],
      url: null,
    });
    await waitFor(() => expect(screen.getByPlaceholderText(t.mcpName)).toHaveValue(""));
    expect(base.refreshMcpConfig).toHaveBeenCalledTimes(1);
    expect(base.refreshSessions).toHaveBeenCalledWith(WORKSPACE);
  });

  it("keeps a double-quoted argument together, so a path with spaces survives", async () => {
    renderModal({
      settingsTab: "mcp",
      mcpForm: { name: "fs", command: "npx", args: '-y server-filesystem "D:\\My Projects\\api"', url: "" },
    });
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpAdd }));
    expect(invoke).toHaveBeenCalledWith("mcp_config_upsert", {
      name: "fs",
      command: "npx",
      args: ["-y", "server-filesystem", "D:\\My Projects\\api"],
      url: null,
    });
  });

  it("adds an HTTP server when only a URL is given", async () => {
    renderModal({
      settingsTab: "mcp",
      mcpForm: { name: "remote", command: "", args: "", url: "https://mcp.example.com/mcp" },
    });
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpAdd }));
    expect(invoke).toHaveBeenCalledWith("mcp_config_upsert", {
      name: "remote",
      command: null,
      args: [],
      url: "https://mcp.example.com/mcp",
    });
  });
});

describe("SettingsModal skills", () => {
  it("opens the editor when a skill row is clicked", async () => {
    const base = makeProps({ settingsTab: "skills", skills: [SKILL] });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByText("PDF tools"));
    expect(base.openSkillEditor).toHaveBeenCalledWith("pdf", "/skills/pdf/SKILL.md");
  });

  it("toggles a skill through the engine and replaces the list with the response", async () => {
    vi.mocked(invoke).mockResolvedValue({
      skills: [{ name: "pdf", short_description: "PDF tools", path: "/skills/pdf/SKILL.md", enabled: false, scope: "user" }],
    });
    const base = makeProps({ settingsTab: "skills", skills: [SKILL] });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpDisable }));
    expect(invoke).toHaveBeenCalledWith("skills_toggle", { name: "pdf", enabled: false, workspace: WORKSPACE });
    expect(base.setSkills).toHaveBeenCalledWith([
      { name: "pdf", description: "PDF tools", path: "/skills/pdf/SKILL.md", enabled: false, scope: "user" },
    ]);
  });

  it("maps a sparse engine response onto the list shape", async () => {
    vi.mocked(invoke).mockResolvedValue({
      skills: [{ name: "doc", description: "Docs", path: "/skills/doc/SKILL.md", scope: 7 }],
    });
    const base = makeProps({ settingsTab: "skills", skills: [SKILL] });
    render(<SettingsModal {...base} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.mcpDisable }));
    expect(base.setSkills).toHaveBeenCalledWith([
      { name: "doc", description: "Docs", path: "/skills/doc/SKILL.md", enabled: true, scope: "" },
    ]);
  });

  it("creates a skill only with a name, then clears the form and refreshes", async () => {
    const base = renderModal({ settingsTab: "skills" });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: t.skillsCreate }));
    expect(invoke).not.toHaveBeenCalled();
    await user.type(screen.getByPlaceholderText(t.skillsName), " pdf ");
    await user.type(screen.getByPlaceholderText(t.skillsDesc), "PDF tools");
    await user.click(screen.getByRole("button", { name: t.skillsCreate }));
    expect(invoke).toHaveBeenCalledWith("skills_create", { name: "pdf", description: "PDF tools" });
    await waitFor(() => expect(screen.getByPlaceholderText(t.skillsName)).toHaveValue(""));
    expect(base.refreshSkills).toHaveBeenCalledTimes(1);
  });

  it("opens the skills folder on demand", async () => {
    render(<SettingsModal {...makeProps({ settingsTab: "skills" })} />);
    await userEvent.setup().click(screen.getByRole("button", { name: t.skillsOpen }));
    expect(invoke).toHaveBeenCalledWith("skills_open");
  });
});

describe("SettingsModal hooks", () => {
  const HOOKS = [
    { event: "PreToolUse", command: "a" },
    { event: "Stop", command: "b" },
  ];

  it("deletes the chosen hook by saving the remaining list", async () => {
    const base = makeProps({ settingsTab: "hooks", hooks: HOOKS });
    render(<SettingsModal {...base} />);
    const deletes = screen.getAllByRole("button", { name: t.mcpDelete });
    expect(deletes).toHaveLength(2);
    await userEvent.setup().click(deletes[1]);
    expect(base.saveHooks).toHaveBeenCalledTimes(1);
    expect(base.saveHooks).toHaveBeenCalledWith([{ event: "PreToolUse", command: "a" }]);
  });

  it("offers exactly the five supported events", () => {
    render(<SettingsModal {...makeProps({ settingsTab: "hooks" })} />);
    const events = within(screen.getByRole("combobox"))
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(events).toEqual(["PreToolUse", "PostToolUse", "SessionStart", "Stop", "UserPromptSubmit"]);
  });

  it("appends a hook with the chosen event, then clears the command", async () => {
    const base = renderModal({ settingsTab: "hooks", hooks: [{ event: "Stop", command: "b" }] });
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox"), "SessionStart");
    await user.type(screen.getByPlaceholderText(t.hooksCommand), "echo hi");
    await user.click(screen.getByRole("button", { name: t.hooksAdd }));
    expect(base.saveHooks).toHaveBeenCalledWith([
      { event: "Stop", command: "b" },
      { event: "SessionStart", command: "echo hi" },
    ]);
    expect(screen.getByPlaceholderText(t.hooksCommand)).toHaveValue("");
    expect(screen.getByRole("combobox")).toHaveValue("SessionStart");
  });

  it("ignores an empty command", async () => {
    const base = renderModal({ settingsTab: "hooks" });
    await userEvent.setup().click(screen.getByRole("button", { name: t.hooksAdd }));
    expect(base.saveHooks).not.toHaveBeenCalled();
  });
});

describe("SettingsModal about", () => {
  it("shows the version, runs an update check, and surfaces the update message", async () => {
    const base = makeProps({ settingsTab: "about" });
    const { rerender } = render(<SettingsModal {...base} />);
    expect(screen.getByText("WanCode v0.23.7")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: t.checkUpdate }));
    expect(base.runUpdate).toHaveBeenCalledTimes(1);
    rerender(<SettingsModal {...makeProps({ settingsTab: "about", updateMsg: "up to date" })} />);
    expect(screen.getByText("up to date")).toBeInTheDocument();
  });

  it("reads execution diagnostics once on open and reports a healthy ledger", async () => {
    vi.mocked(invoke).mockResolvedValue(DIAGNOSTICS);
    render(<SettingsModal {...makeProps({ settingsTab: "about" })} />);
    expect(await screen.findByText(t.executionIntegrityHealthy)).toBeInTheDocument();
    expect(
      screen.getByText(t.executionIntegritySummary(DIAGNOSTICS.event_count, 2, 0, "abcdef123456")),
    ).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("agent_execution_diagnostics");
  });

  it("flags duplicate events and re-checks on demand", async () => {
    vi.mocked(invoke).mockResolvedValue({ ...DIAGNOSTICS, duplicate_event_ids: ["e1"] });
    render(<SettingsModal {...makeProps({ settingsTab: "about" })} />);
    await screen.findByText(t.executionIntegrityBlocked);
    await userEvent.setup().click(screen.getByRole("button", { name: t.executionIntegrityRefresh }));
    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledTimes(2));
  });
});
