const WORK_SESSION_KEY = "wancode-work-session";
const WORK_WORKSPACE_KEY = "wancode-work-workspace";

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function loadWorkSession(storage: StorageLike): string | undefined {
  const value = storage.getItem(WORK_SESSION_KEY)?.trim();
  return value || undefined;
}

export function rememberWorkSession(storage: StorageLike, sessionId: string): void {
  const value = sessionId.trim();
  if (value) storage.setItem(WORK_SESSION_KEY, value);
}

export function forgetWorkSession(storage: StorageLike): void {
  storage.removeItem(WORK_SESSION_KEY);
}

export function loadWorkWorkspace(storage: StorageLike): string | undefined {
  const value = storage.getItem(WORK_WORKSPACE_KEY)?.trim();
  return value || undefined;
}

export function rememberWorkWorkspace(storage: StorageLike, workspaceId: string): void {
  const value = workspaceId.trim();
  if (value) storage.setItem(WORK_WORKSPACE_KEY, value);
}

export function forgetWorkWorkspace(storage: StorageLike): void {
  storage.removeItem(WORK_WORKSPACE_KEY);
}
