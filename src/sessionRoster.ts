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
