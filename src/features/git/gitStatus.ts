/** Normalize Tauri / engine error payloads into a comparable string. */
export function gitErrorText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object") {
    const o = err as Record<string, unknown>;
    if (typeof o.message === "string") return o.message;
    if (typeof o.error === "string") return o.error;
  }
  return String(err ?? "");
}

/**
 * Engine/client "not a git repo" failures. Match both the i18n string
 * (capital Git) and the ext_call reject ("不是 git 仓库", lowercase).
 */
export function isNotGitRepoError(err: unknown): boolean {
  const s = gitErrorText(err);
  return /不是\s*git\s*仓库/i.test(s) || /not a git (repo|repository)/i.test(s);
}

/**
 * Capability-lease denials for read-class git extensions: the live session's
 * surface lease has no `read` tool (Chat is zero-file-surface), so the host
 * ext call is rejected with
 * "CAPABILITY_EXTENSION_BLOCKED: x.ai/git/…: tool is denied: read."
 * (and the fs/path variant CAPABILITY_PATH_BLOCKED). Callers should show the
 * localized "switch to Code" hint instead of the raw engine error. This is
 * policy working as designed — not an engine failure.
 */
export function isCapabilityDeniedError(err: unknown): boolean {
  return /CAPABILITY_(EXTENSION|PATH)_BLOCKED/.test(gitErrorText(err));
}

/** Diffs envelope: null data = not a repo; files[] = repo (possibly clean). */
export function parseGitDiffsFiles(payload: unknown): unknown[] | null | undefined {
  const r = payload as Record<string, unknown> | null | undefined;
  if (r?.error) return undefined;
  const env = (r?.result ?? r) as Record<string, unknown> | null | undefined;
  const d = (env?.data ?? env) as Record<string, unknown> | null | undefined;
  if (!d || d.files == null) return null;
  return Array.isArray(d.files) ? d.files : null;
}
