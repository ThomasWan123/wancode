/* v0.18.6 步15：模型阻塞的 UI 状态机回归门。
 *
 * 为什么必须有这层：这几轮外部复核抓到的问题——"稍后再说"假解除、
 * model_unavailable 掉进无反馈死区、切换会话残留旧阻塞、候选被 filter
 * 悄悄丢掉——全都是 TypeScript 类型检查和引擎集成测试结构上抓不到的。
 * 引擎那边可以完全正确，用户仍然卡在一个点了没反应的按钮上。
 *
 * RTL 是回归门，真机 dogfooding 是最终体验验收，两者不互相替代。 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { Composer } from "./Composer";
import { STRINGS } from "../../i18n";
import type { ModelBlock } from "../../modelBlock";

const t = STRINGS.zh;

const AMBIGUOUS: ModelBlock = {
  kind: "ambiguous_model_id",
  requested: "glm-4.6",
  candidates: [
    { id: "glm-open", name: "智谱开放平台", endpointLabel: "open.bigmodel.cn", selectable: true },
    { id: "glm-coding", name: "Coding Plan", endpointLabel: "coding.bigmodel.cn", selectable: true },
  ],
};

/** Composer 的 props 是 Record<string, any>，这里只给渲染必需的那些。 */
function renderComposer(overrides: Record<string, any> = {}) {
  const setModelBlock = vi.fn();
  const setModelBlockOpen = vi.fn();
  const props: Record<string, any> = {
    MODE_ORDER: ["default"],
    busy: false,
    draftRef: { current: "" },
    histIdxRef: { current: -1 },
    historyRef: { current: [] },
    fileInputRef: { current: null },
    taRef: { current: null },
    input: "你好",
    lang: "zh",
    model: "glm-open",
    models: ["glm-open", "glm-coding"],
    modeMeta: { default: { label: "默认", desc: "" } },
    modeMenu: false,
    pastedImages: [],
    permMode: "default",
    popup: null,
    popupItems: [],
    queue: [],
    sessionId: "s1",
    starting: false,
    workspace: "D:/proj",
    t,
    modelBlock: AMBIGUOUS,
    modelBlockOpen: true,
    setModelBlock,
    setModelBlockOpen,
    setError: vi.fn(),
    setInput: vi.fn(),
    setItems: vi.fn(),
    setMode: vi.fn(),
    setModeMenu: vi.fn(),
    setModel: vi.fn(),
    setPastedImages: vi.fn(),
    setPlusMenu: vi.fn(),
    setPopup: vi.fn(),
    setEditingQueueId: vi.fn(),
    setSettingsTab: vi.fn(),
    setShowSettings: vi.fn(),
    setShowTerminal: vi.fn(),
    send: vi.fn(),
    sendInterject: vi.fn(),
    onComposerChange: vi.fn(),
    onPaste: vi.fn(),
    onPickImages: vi.fn(),
    acceptPopup: vi.fn(),
    pickFolderAndConnect: vi.fn(),
    refreshMcpConfig: vi.fn(),
    ...overrides,
  };
  const utils = render(<Composer {...props} />);
  return { ...utils, setModelBlock, setModelBlockOpen, props };
}

const sendButton = () => screen.getByTitle(new RegExp(`${t.send}|${t.ambiguousBlocked}`));

beforeEach(() => invokeMock.mockReset());

describe("模型阻塞的 UI 状态机", () => {
  it("歧义恢复时列出全部候选及其端点，并禁用发送", () => {
    renderComposer();
    expect(screen.getByText("智谱开放平台")).toBeInTheDocument();
    expect(screen.getByText("Coding Plan")).toBeInTheDocument();
    // 端点是区分同名模型的唯一依据——曾因 snake_case/camelCase 错配而全空。
    expect(screen.getByText("open.bigmodel.cn")).toBeInTheDocument();
    expect(screen.getByText("coding.bigmodel.cn")).toBeInTheDocument();
    expect(sendButton()).toBeDisabled();
  });

  it("稍后再说只收起弹窗，绝不清除会话级阻塞", async () => {
    const user = userEvent.setup();
    const { setModelBlockOpen, setModelBlock } = renderComposer();
    await user.click(screen.getByText(t.ambiguousDismiss));
    expect(setModelBlockOpen).toHaveBeenCalledWith(false);
    // 收起不是解除。清掉 modelBlock 会让前端与引擎侧脱节，用户点发送只
    // 收到一个空 EndTurn——比不给这个按钮更糟。
    expect(setModelBlock).not.toHaveBeenCalled();
  });

  it("收起后提示条仍在、发送仍禁用、可重新展开", async () => {
    const user = userEvent.setup();
    const { container, setModelBlockOpen } = renderComposer({ modelBlockOpen: false });
    expect(screen.queryByText("智谱开放平台")).not.toBeInTheDocument();
    expect(screen.getByText(new RegExp(t.ambiguousBlocked))).toBeInTheDocument();
    expect(container.querySelector(".send-btn")).toBeDisabled();
    await user.click(screen.getByText(new RegExp(t.ambiguousReopen)));
    expect(setModelBlockOpen).toHaveBeenCalledWith(true);
  });

  it("点选候选时传给引擎的是 catalog key，而不是共享的 slug", async () => {
    const user = userEvent.setup();
    invokeMock.mockResolvedValue(undefined);
    const { setModelBlock } = renderComposer();
    await user.click(screen.getByText("Coding Plan"));
    expect(invokeMock).toHaveBeenCalledWith("agent_set_model", { model: "glm-coding" });
    // 切换成功才解除阻塞。
    expect(setModelBlock).toHaveBeenCalledWith(null);
  });

  it("不可用类阻塞可显式确认当前模型——只剩一个 option 时也不会死锁", async () => {
    const user = userEvent.setup();
    invokeMock.mockResolvedValue(undefined);
    const { setModelBlock } = renderComposer({
      modelBlock: { kind: "model_unavailable", requested: "gone-model" } as ModelBlock,
      model: "only-model",
      models: ["only-model"],
    });
    expect(screen.getByText(t.unavailableTitle)).toBeInTheDocument();
    expect(screen.getByText("gone-model")).toBeInTheDocument();
    expect(sendButton()).toBeDisabled();
    await user.click(screen.getByText(t.unavailableUseCurrent));
    expect(invokeMock).toHaveBeenCalledWith("agent_set_model", { model: "only-model" });
    expect(setModelBlock).toHaveBeenCalledWith(null);
  });

  it("读不懂的阻塞同样给出说明并禁用发送", () => {
    renderComposer({ modelBlock: { kind: "unknown", raw: "future_reason" } as ModelBlock });
    expect(screen.getByText(t.blockUnknownTitle)).toBeInTheDocument();
    expect(sendButton()).toBeDisabled();
  });

  it("无阻塞时发送可用，且没有任何阻塞 UI", () => {
    renderComposer({ modelBlock: null });
    expect(sendButton()).toBeEnabled();
    expect(screen.queryByText(t.ambiguousTitle)).not.toBeInTheDocument();
    expect(screen.queryByText(new RegExp(t.ambiguousBlocked))).not.toBeInTheDocument();
  });
});
