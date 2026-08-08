export type SurfaceKind = "chat" | "code";

export function parseSurface(value: unknown): SurfaceKind {
  return value === "chat" ? "chat" : "code";
}

/** A layer switch never reassigns an existing session; it prepares a new one. */
export function surfaceSwitchRequiresNewSession(
  current: SurfaceKind,
  next: SurfaceKind,
  sessionId: string,
): boolean {
  return current !== next && sessionId.length > 0;
}
