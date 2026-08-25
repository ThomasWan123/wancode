/** Work talks about ordinary files in an opened folder — not a quarantined import library. */

export type WorkDocKind = "pdf" | "docx" | "xlsx";

export const WORK_DOC_EXTENSIONS: readonly WorkDocKind[] = ["pdf", "docx", "xlsx"];

export function workDocKind(path: string): WorkDocKind | null {
  const base = fileBaseName(path);
  const dot = base.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = base.slice(dot + 1).toLowerCase();
  if (ext === "pdf" || ext === "docx" || ext === "xlsx") return ext;
  return null;
}

export function isWorkDocument(path: string): boolean {
  return workDocKind(path) !== null;
}

/** PDF / DOCX still feed the existing parse pipeline; Excel is a normal folder file. */
export function canParseForWorkContext(kind: WorkDocKind | null): boolean {
  return kind === "pdf" || kind === "docx";
}

export function workDeskFiles(fileList: string[]): { path: string; kind: WorkDocKind }[] {
  const out: { path: string; kind: WorkDocKind }[] = [];
  for (const path of fileList) {
    const kind = workDocKind(path);
    if (kind) out.push({ path, kind });
  }
  return out;
}

export function fileBaseName(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).pop() || path;
}

export function folderBaseName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).filter(Boolean).pop() || path;
}

export function joinWorkspacePath(workspace: string, rel: string): string {
  const normWs = workspace.replace(/[\\/]+$/, "");
  const normRel = rel.replace(/\\/g, "/").replace(/^\/+/, "");
  const sep = workspace.includes("\\") ? "\\" : "/";
  return `${normWs}${sep}${normRel.replace(/\//g, sep)}`;
}

/** Tauri/WebView2 file drops expose an absolute `path` on the File object. */
export function pathsFromDataTransfer(
  dt: { files?: ArrayLike<File> | FileList | null } | null | undefined,
): string[] {
  if (!dt?.files) return [];
  const out: string[] = [];
  const files = Array.from(dt.files as ArrayLike<File>);
  for (const file of files) {
    const path = (file as File & { path?: string }).path;
    if (typeof path === "string" && path.trim()) out.push(path.trim());
  }
  return out;
}
