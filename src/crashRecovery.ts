export type CrashRecoveryInfo = {
  sessionId: string;
  workspace: string;
};

type CrashRecoveryDeps = {
  startSession: (sessionId: string, workspace?: string) => Promise<string>;
  acknowledge: () => Promise<unknown>;
};

/**
 * Resolve the crash banner without losing the only recovery pointer.
 *
 * Restore deliberately does not acknowledge the old marker first. A successful
 * session start writes the active session's dirty marker itself; a failed start
 * must leave the old marker intact so the next launch can offer recovery again.
 */
export async function resolveCrashRecovery(
  action: "restore" | "dismiss",
  info: CrashRecoveryInfo,
  deps: CrashRecoveryDeps,
): Promise<boolean> {
  if (action === "restore") {
    return Boolean(await deps.startSession(info.sessionId, info.workspace || undefined));
  }
  await deps.acknowledge();
  return true;
}
