// 前端会话层身份。与后端 SurfaceKind 对齐(surface.rs):v0.20 W2 起 Work 为
// 一等前端层。Cowork 待 Cowork 线落地再加(此处不预置未接线的层)。
export type SurfaceKind = "chat" | "code" | "work";

const KNOWN: readonly SurfaceKind[] = ["chat", "code", "work"];

/** 解析持久化/回传的层值;未知一律 fail 回 Code(不猜测)。 */
export function parseSurface(value: unknown): SurfaceKind {
  return typeof value === "string" && (KNOWN as readonly string[]).includes(value)
    ? (value as SurfaceKind)
    : "code";
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
