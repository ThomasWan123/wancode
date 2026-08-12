// 前端会话层身份。与后端 SurfaceKind 对齐(surface.rs):v0.20 W2 起 Work 为
// 一等前端层。Cowork 待 Cowork 线落地再加(此处不预置未接线的层)。
export type SurfaceKind = "chat" | "code" | "work";

const KNOWN: readonly SurfaceKind[] = ["chat", "code", "work"];

/** Work 层 UI 是否已全量接线(switcher/视图/导入/会话创建)。**W2-fe-b 前为
 *  false**:模型认得 Work,但激活路径(见 resolveActiveSurface)不得让它成为
 *  当前层——否则其余 UI 会 fall through 到 Code 行为、且启动会向后端传 Work
 *  而被 bind_new_session 拒(codex W2-fe-a R1)。W2-fe-b 落地时翻成 true。 */
export const WORK_UI_READY = false;

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

/** 换层从不改写既有会话,只准备新会话。 */
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
