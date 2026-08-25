// 前端会话层身份。与后端 SurfaceKind 对齐(surface.rs):v0.20 W2 起 Work 为
// 一等前端层。Cowork 待 Cowork 线落地再加(此处不预置未接线的层)。
export type SurfaceKind = "chat" | "code" | "work";

const KNOWN: readonly SurfaceKind[] = ["chat", "code", "work"];

/** Work 层 UI 是否已全量接线(switcher/视图/导入/会话创建)。**W2-fe-b 起为
 *  true**:switcher 有 Work 按钮、有 Work 视图与导入、agent_start 能创建 Work
 *  会话。与**后端 agent.rs 的 surface_launchable 协同**——两处必须同版本一起
 *  放行 Work(否则半接线:一边允许启动、另一边拒绝显示 → 孤儿会话)。 */
export const WORK_UI_READY = true;

/** 解析持久化/回传的层值为**合法** SurfaceKind;未知一律 fail 回 Code(不猜测)。
 *  这是纯校验,供 W2-fe-b 等所有消费者使用;是否可**激活**另见
 *  resolveActiveSurface。 */
export function parseSurface(value: unknown): SurfaceKind {
  return typeof value === "string" && (KNOWN as readonly string[]).includes(value)
    ? (value as SurfaceKind)
    : "code";
}

/** 解析出**当前可激活**的层:合法 + 已接线。Work 在 UI 未就绪时降级回 Code,
 *  使持久化/后端回传的 Work 不会激活半接线状态(fail-closed)。App 的两个激活
 *  点(localStorage 初始化、后端返回 surface)都必须走这里,而非裸 parseSurface。 */
export function resolveActiveSurface(value: unknown): SurfaceKind {
  const parsed = parseSurface(value);
  if (parsed === "work" && !WORK_UI_READY) return "code";
  return parsed;
}

/** 换层从不改写既有会话的层身份（引擎 SurfaceBinding 不可变），只准备新会话。
 *  UI 必须保留上一层的 transcript 并在目标层重连，而不是看起来像硬重置。 */
export function surfaceSwitchRequiresNewSession(
  current: SurfaceKind,
  next: SurfaceKind,
  sessionId: string,
): boolean {
  return current !== next && sessionId.length > 0;
}

/** 该层的新会话是否需要一个 Work 工作区身份(仅 Work)。Work 会话创建时
 *  必须携带 workspace_id(后端 bind_new_work_session / 身份不变量)。 */
export function surfaceNeedsWorkspace(kind: SurfaceKind): boolean {
  return kind === "work";
}

/** Localized top-bar label. Default language is zh (聊天 / 代码 / 工作). */
export function surfaceLabel(
  kind: SurfaceKind,
  t: { surfaceChat: string; surfaceCode: string; surfaceWork: string },
): string {
  if (kind === "chat") return t.surfaceChat;
  if (kind === "code") return t.surfaceCode;
  return t.surfaceWork;
}

/** 前端**已全链路接线**、可作为当前层激活的 surface。Work 待 W2-fe-b、
 *  Cowork 待 Cowork 线。后端 agent.rs 的 surface_launchable 与此协同。 */
const WIRED: readonly SurfaceKind[] = WORK_UI_READY ? ["chat", "code", "work"] : ["chat", "code"];

/** 后端**已启动**会话回传 surface 后的激活决策(codex W2-fe-a R2/R3)。与
 *  resolveActiveSurface(启动前 localStorage 偏好,降级即可)不同:后端返回值
 *  已带活会话,任何**未接线的层**(Work、Cowork,以及未来任何后端有而前端
 *  没接的层)都不能降级为 Code 掩盖身份——必须 reject,由调用方 fail closed。
 *  区分"已知但未接线的层"与"畸形/未知输入",便于报错。返回判别式,可单测。
 *
 *  注:真正的防线在后端(agent.rs surface_launchable 在发布 handle 前拦截,
 *  故后端根本不会带着活 handle 返回这些层);本函数是前端纵深防御。 */
export type BackendSurfaceDecision =
  | { activate: true; surface: SurfaceKind }
  | { activate: false; reason: "layer-not-wired" | "unknown-surface" };

const BACKEND_KNOWN: readonly string[] = ["chat", "code", "work", "cowork"];

export function decideBackendSurface(value: unknown): BackendSurfaceDecision {
  // 已接线层 → 激活。
  if (typeof value === "string" && (WIRED as readonly string[]).includes(value)) {
    return { activate: true, surface: value as SurfaceKind };
  }
  // 后端已知但前端未接线的层(work/cowork/…) → reject(不降级掩盖身份)。
  if (typeof value === "string" && BACKEND_KNOWN.includes(value)) {
    return { activate: false, reason: "layer-not-wired" };
  }
  // 畸形/未知输入 → reject(不猜测、不激活)。
  return { activate: false, reason: "unknown-surface" };
}
