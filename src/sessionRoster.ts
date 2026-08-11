// 会话列表(roster)刷新的目标选择与并发守卫。
//
// 为什么独立成模块:sessions/changed 监听器安装于挂载时,闭包捕获的
// workspace/surface 是旧值——Chat 界面下一次晚到的 Code 工作区响应会把
// 修好的 Chat 列表覆盖回去,且 Chat 会话激活期间不再自愈(PR #38 F1)。
// 目标选择必须在【执行时】按当前 surface 决定,并可被单测锁死。

/** Chat 界面的目标是后端解析的私有工作区,用哨兵值表达,由调用方换取真实路径。 */
export const CHAT_WORKSPACE = "chat-workspace" as const;

export type RosterTarget = typeof CHAT_WORKSPACE | string | null;

/**
 * 执行时按当前 surface 选刷新目标:
 * - chat → CHAT_WORKSPACE 哨兵(调用方 invoke chat_workspace 换真实路径);
 * - 其余 → 当前 Code 工作区;没有工作区则不刷新(null)。
 */
export function rosterRefreshTarget(
  surface: string,
  codeWorkspace: string | null | undefined,
): RosterTarget {
  if (surface === "chat") return CHAT_WORKSPACE;
  return codeWorkspace || null;
}

export type RosterCoordinatorDeps = {
  /** 执行时读当前 surface(必须是 ref 读取,不是闭包值) */
  getSurface: () => string;
  /** 执行时读当前 Code 工作区 */
  getCodeWorkspace: () => string | null | undefined;
  /** 解析 Chat 私有工作区(后端 chat_workspace 命令) */
  resolveChatWorkspace: () => Promise<string>;
  /** 实际刷新(内部带代数守卫) */
  refresh: (ws: string) => Promise<void>;
  /** 清空可见列表(仅 Chat 入口解析失败且要求 clear 时) */
  clearRoster: () => void;
};

/**
 * 生产协调器:所有会话列表刷新(入口 effect 与 sessions/changed 监听)
 * 必须经它执行。两个方向的竞态都在这里堵死:
 * - 正向(F1 原报):Chat 界面下晚到的 Code 响应不得覆盖 Chat 列表——
 *   目标在执行时按当前 surface 选,chat 目标与 Code 工作区无关;
 * - 反向(round-2 F1):陈旧的 Chat 解析在用户切回 Code 后释放,不得
 *   覆盖 Code 列表——resolve await 之后**复查 surface**,已切走即丢弃,
 *   绝不领取新代数。
 */
export function createRosterCoordinator(deps: RosterCoordinatorDeps) {
  return async function refreshForCurrentSurface(opts?: {
    /** Chat 解析失败时:入口语义用 "clear"(不能展示外来列表),通知语义用 "keep" */
    onChatResolveFailure?: "clear" | "keep";
  }): Promise<void> {
    const target = rosterRefreshTarget(deps.getSurface(), deps.getCodeWorkspace());
    if (target === CHAT_WORKSPACE) {
      let chatWs: string;
      try {
        chatWs = await deps.resolveChatWorkspace();
      } catch {
        if ((opts?.onChatResolveFailure ?? "keep") === "clear") deps.clearRoster();
        return;
      }
      if (deps.getSurface() !== "chat") return; // 反向竞态:await 期间已切走
      await deps.refresh(chatWs);
    } else if (target) {
      await deps.refresh(target);
    }
  };
}

/**
 * 单调代数守卫:后发起的刷新永远胜过先发起的慢响应。
 * begin() 领取代数;isCurrent(g) 在写状态前校验——旧代数一律丢弃。
 */
export function createRefreshGuard() {
  let generation = 0;
  return {
    begin(): number {
      return ++generation;
    },
    isCurrent(g: number): boolean {
      return g === generation;
    },
  };
}
