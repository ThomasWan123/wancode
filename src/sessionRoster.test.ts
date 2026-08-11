import { describe, expect, it } from "vitest";
import {
  CHAT_WORKSPACE,
  createRefreshGuard,
  createRosterCoordinator,
  rosterRefreshTarget,
} from "./sessionRoster";

// 测试驱动的是生产协调器本体(App.tsx 的入口 effect 与 sessions/changed
// 监听调用的就是它),不是平行实现。夹具只替换 IO 依赖。
function makeFixture(opts?: { resolveChat?: () => Promise<string> }) {
  const state = {
    surface: "code" as string,
    codeWorkspace: "D:/proj" as string | null,
    visibleRoster: "initial" as string,
    cleared: 0,
  };
  const guard = createRefreshGuard();
  const refresh = async (ws: string) => {
    const g = guard.begin();
    await Promise.resolve(); // 模拟 IO 一跳
    if (!guard.isCurrent(g)) return;
    state.visibleRoster = ws === "CHAT-WS" ? "chat-roster" : "code-roster";
  };
  const coordinator = createRosterCoordinator({
    getSurface: () => state.surface,
    getCodeWorkspace: () => state.codeWorkspace,
    resolveChatWorkspace: opts?.resolveChat ?? (() => Promise.resolve("CHAT-WS")),
    refresh,
    clearRoster: () => {
      state.cleared++;
      state.visibleRoster = "empty";
    },
  });
  return { state, coordinator, refresh };
}

describe("rosterRefreshTarget", () => {
  it("code surface + 工作区 → 该工作区;无工作区 → null", () => {
    expect(rosterRefreshTarget("code", "D:/proj")).toBe("D:/proj");
    expect(rosterRefreshTarget("code", null)).toBeNull();
  });
  it("chat surface → 哨兵,与 Code 工作区无关", () => {
    expect(rosterRefreshTarget("chat", "D:/proj")).toBe(CHAT_WORKSPACE);
  });
});

describe("createRosterCoordinator — 生产协调器", () => {
  it("Code 正对照:code surface 下刷新 Code 工作区", async () => {
    const { state, coordinator } = makeFixture();
    await coordinator();
    expect(state.visibleRoster).toBe("code-roster");
  });

  it("Chat 正对照:chat surface 下刷新 Chat 私有工作区", async () => {
    const { state, coordinator } = makeFixture();
    state.surface = "chat";
    await coordinator();
    expect(state.visibleRoster).toBe("chat-roster");
  });

  it("原事故时序:Chat 刷新后 sessions/changed 到达 → 目标仍是 Chat,Code 工作区被无视", async () => {
    // round-1 前的监听器用闭包捕获的 Code 工作区刷新——这里若协调器
    // 在 chat surface 下用了 codeWorkspace,可见列表会变 code-roster。
    const { state, coordinator } = makeFixture();
    state.surface = "chat";
    await coordinator(); // Chat 入口刷新
    await coordinator(); // sessions/changed 再触发
    expect(state.visibleRoster).toBe("chat-roster");
  });

  it("反向竞态:陈旧 Chat 解析在切回 Code 后释放 → 必须丢弃,最终列表是 Code", async () => {
    // round-2 F1:入口 effect 的 .then 无条件 refreshSessions(chatWs),
    // 陈旧延续领取新代数反而必胜。协调器在 await 后复查 surface 堵死。
    let releaseChat!: (ws: string) => void;
    const held = new Promise<string>((r) => (releaseChat = r));
    const { state, coordinator } = makeFixture({ resolveChat: () => held });

    state.surface = "chat";
    const staleChatEntry = coordinator({ onChatResolveFailure: "clear" }); // 挂住

    state.surface = "code";
    await coordinator(); // 用户切回 Code 并完成刷新
    expect(state.visibleRoster).toBe("code-roster");

    releaseChat("CHAT-WS"); // 陈旧 Chat 解析此刻才返回
    await staleChatEntry;
    expect(state.visibleRoster).toBe("code-roster"); // 不得被覆盖
  });

  it("Chat 入口解析失败 → clear 语义清空;通知语义保持现状", async () => {
    const failing = () => Promise.reject(new Error("backend down"));
    const a = makeFixture({ resolveChat: failing });
    a.state.surface = "chat";
    a.state.visibleRoster = "code-roster";
    await a.coordinator({ onChatResolveFailure: "clear" });
    expect(a.state.cleared).toBe(1);
    expect(a.state.visibleRoster).toBe("empty");

    const b = makeFixture({ resolveChat: failing });
    b.state.surface = "chat";
    b.state.visibleRoster = "chat-roster";
    await b.coordinator(); // 默认 keep
    expect(b.state.cleared).toBe(0);
    expect(b.state.visibleRoster).toBe("chat-roster");
  });

  it("失败路径孪生竞态:陈旧 Chat 解析在切回 Code 后 reject → 不得清空 Code 列表", async () => {
    // round-3 F1:成功路径复查了 surface,失败路径若不复查,被 reject 的
    // 旧 Chat 入口调用会把正确的 Code 列表清成空。
    let rejectChat!: (e: Error) => void;
    const held = new Promise<string>((_, rej) => (rejectChat = rej));
    const { state, coordinator } = makeFixture({ resolveChat: () => held });

    state.surface = "chat";
    const staleChatEntry = coordinator({ onChatResolveFailure: "clear" }); // 挂住

    state.surface = "code";
    await coordinator(); // Code 正对照完成
    expect(state.visibleRoster).toBe("code-roster");

    rejectChat(new Error("backend down")); // 陈旧解析此刻才失败
    await staleChatEntry;
    expect(state.cleared).toBe(0); // 不得清空
    expect(state.visibleRoster).toBe("code-roster");
  });
});

describe("createRefreshGuard", () => {
  it("慢的旧代数不得覆盖新代数", async () => {
    const guard = createRefreshGuard();
    let visible = "initial";
    const gOld = guard.begin();
    const slow = (async () => {
      await new Promise((r) => setTimeout(r, 20));
      if (guard.isCurrent(gOld)) visible = "stale";
    })();
    const gNew = guard.begin();
    if (guard.isCurrent(gNew)) visible = "fresh";
    await slow;
    expect(visible).toBe("fresh");
  });
});
