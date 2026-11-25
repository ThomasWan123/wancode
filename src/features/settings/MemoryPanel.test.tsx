/* MemoryPanel 的特征测试：开关、蒸馏、追加各自打哪条 Tauri 命令，
   以及仅 Code 层可用的门。 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { STRINGS } from "../../i18n";
import { MemoryPanel } from "./MemoryPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const t = STRINGS.en;
const WORKSPACE = "D:\\proj";

/** 每条记忆命令的默认应答；单测按需覆盖其中一条。
    覆盖值是函数时在**被调用那一刻**才求值——预先造好的 rejected Promise
    会在组件来接之前就被判成 unhandled rejection。 */
function mockMemory(overrides: Record<string, unknown> = {}) {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    const replies: Record<string, unknown> = {
      memory_config_get: true,
      memory_read_global: "global text",
      memory_read_workspace: { dir_name: "proj-1a2b3c4d", content: "workspace text" },
      ...overrides,
    };
    const reply = replies[cmd];
    return typeof reply === "function" ? reply() : reply;
  });
}

function renderPanel(props: Partial<Parameters<typeof MemoryPanel>[0]> = {}) {
  render(<MemoryPanel sessionId="s1" surface="code" workspace={WORKSPACE} t={t} {...props} />);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
});

describe("MemoryPanel", () => {
  it("is Code-only: other surfaces get an explanation instead of controls", () => {
    renderPanel({ surface: "chat" });
    expect(screen.getByText(t.memCodeOnly)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("loads the switch and both memory files", async () => {
    mockMemory();
    renderPanel();
    expect(await screen.findByText("global text")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: t.memEnable })).toBeChecked();
    expect(screen.getByText("proj-1a2b3c4d")).toBeInTheDocument();
    expect(screen.getByText("workspace text")).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("memory_read_workspace", { workspace: WORKSPACE });
  });

  it("reads nothing for a workspace it does not have", async () => {
    mockMemory();
    renderPanel({ workspace: "" });
    await screen.findByText("global text");
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("memory_read_workspace", expect.anything());
    expect(screen.getByText(t.memWorkspaceNone)).toBeInTheDocument();
  });

  it("shows the empty marker for each missing file", async () => {
    mockMemory({
      memory_read_global: "",
      memory_read_workspace: { dir_name: "proj-1a2b3c4d", content: "" },
    });
    renderPanel();
    await screen.findByText(t.memEmpty);
    expect(screen.getAllByText(t.memEmpty)).toHaveLength(2);
  });

  it("writes the switch and echoes the new-session note as action feedback", async () => {
    mockMemory();
    renderPanel();
    const box = await screen.findByRole("checkbox", { name: t.memEnable });
    // 这句说明常驻在开关下方；切换成功后同一段文字再作为操作反馈出现一次。
    expect(screen.getAllByText(t.memEnableNote)).toHaveLength(1);
    await userEvent.setup().click(box);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("memory_config_set", { enabled: false });
    await waitFor(() => expect(screen.getAllByText(t.memEnableNote)).toHaveLength(2));
    expect(box).not.toBeChecked();
  });

  it("restores the switch when the write fails", async () => {
    mockMemory({ memory_config_set: () => Promise.reject("disk full") });
    renderPanel();
    await screen.findByText("global text");
    await userEvent.setup().click(screen.getByRole("checkbox", { name: t.memEnable }));
    expect(await screen.findByText("disk full")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: t.memEnable })).toBeChecked();
  });

  it("needs a session before it can flush", async () => {
    mockMemory();
    renderPanel({ sessionId: "" });
    await screen.findByText("global text");
    expect(screen.getByRole("button", { name: t.memFlush })).toBeDisabled();
    expect(screen.getByText(t.memNoSession)).toBeInTheDocument();
  });

  it("flushes the current session and reloads the files", async () => {
    mockMemory();
    renderPanel();
    await screen.findByText("global text");
    await userEvent.setup().click(screen.getByRole("button", { name: t.memFlush }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("memory_flush");
    expect(await screen.findByText(t.memFlushOk)).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("memory_read_global");
  });

  it("appends a trimmed note to the global file and clears the box", async () => {
    mockMemory();
    renderPanel();
    await screen.findByText("global text");
    const user = userEvent.setup();
    // 占位文本是多行的，按文本查询在这个测试库版本里匹配不上；断言属性本身。
    const box = screen.getByRole("textbox");
    expect(box).toHaveAttribute("placeholder", t.memAddPlaceholder);
    expect(screen.getByRole("button", { name: t.memAppend })).toBeDisabled();
    await user.type(box, "  use pnpm  ");
    await user.click(screen.getByRole("button", { name: t.memAppend }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("memory_append_global", { text: "use pnpm" });
    await waitFor(() => expect(box).toHaveValue(""));
    expect(screen.getByText(t.memAppended)).toBeInTheDocument();
  });
});
