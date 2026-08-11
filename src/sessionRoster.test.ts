import { describe, expect, it } from "vitest";
import { CHAT_WORKSPACE, createRefreshGuard, rosterRefreshTarget } from "./sessionRoster";

describe("rosterRefreshTarget — 执行时按 surface 选目标", () => {
  it("Code 正对照:code surface + 工作区 → 刷新该工作区", () => {
    expect(rosterRefreshTarget("code", "D:/proj")).toBe("D:/proj");
  });

  it("code surface 无工作区 → 不刷新", () => {
    expect(rosterRefreshTarget("code", "")).toBeNull();
    expect(rosterRefreshTarget("code", null)).toBeNull();
  });

  it("chat surface → 永远是 Chat 私有工作区哨兵,绝不落到 Code 工作区", () => {
    // PR #38 F1 的事故形态:监听器闭包里捕获的是 Code 工作区,Chat 界面下
    // sessions/changed 一来就用它刷新,覆盖修好的 Chat 列表。修复后 chat
    // surface 的目标与 Code 工作区无关。
    expect(rosterRefreshTarget("chat", "D:/proj")).toBe(CHAT_WORKSPACE);
    expect(rosterRefreshTarget("chat", null)).toBe(CHAT_WORKSPACE);
  });
});

describe("createRefreshGuard — 对抗性时序", () => {
  it("Chat 刷新 → sessions/changed → 慢 Code 响应:最终可见列表必须仍是 Chat", async () => {
    const guard = createRefreshGuard();
    let visible = "initial";

    // 慢 Code 请求先发起(旧代数)
    const gCode = guard.begin();
    const slowCode = (async () => {
      await new Promise((r) => setTimeout(r, 20));
      if (guard.isCurrent(gCode)) visible = "code-roster";
    })();

    // Chat 刷新后发起(新代数),先返回
    const gChat = guard.begin();
    if (guard.isCurrent(gChat)) visible = "chat-roster";

    await slowCode;
    expect(visible).toBe("chat-roster");
  });

  it("正常顺序不受守卫误伤:后发起者照常落地", () => {
    const guard = createRefreshGuard();
    const g1 = guard.begin();
    expect(guard.isCurrent(g1)).toBe(true);
    const g2 = guard.begin();
    expect(guard.isCurrent(g1)).toBe(false);
    expect(guard.isCurrent(g2)).toBe(true);
  });
});
