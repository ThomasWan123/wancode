/** Work talks about ordinary files in an opened folder — not a quarantined import library. */

import { isWorkImageKind, WORK_DOCUMENT_EXTENSIONS } from "./workFormats";

export type WorkDocKind = (typeof WORK_DOCUMENT_EXTENSIONS)[number];

export { isWorkImageKind, WORK_DOCUMENT_EXTENSIONS };

const WORK_KIND_SET = new Set<string>(WORK_DOCUMENT_EXTENSIONS);

export function workDocKind(path: string): WorkDocKind | null {
  const base = fileBaseName(path);
  const dot = base.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = base.slice(dot + 1).toLowerCase();
  if (WORK_KIND_SET.has(ext)) return ext as WorkDocKind;
  return null;
}

export function isWorkDocument(path: string): boolean {
  return workDocKind(path) !== null;
}

export function isLegacyOffice(path: string): boolean {
  const base = fileBaseName(path);
  const dot = base.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = base.slice(dot + 1).toLowerCase();
  return ext === "doc" || ext === "xls" || ext === "ppt";
}

/** Text parsers run at send; images take the vision path instead. */
export function canParseForWorkContext(kind: WorkDocKind | null): boolean {
  return kind === "pdf" || kind === "docx" || kind === "xlsx" || kind === "pptx";
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

function mentionPresent(text: string, token: string): boolean {
  let from = 0;
  while (from < text.length) {
    const idx = text.indexOf(token, from);
    if (idx < 0) return false;
    const after = text[idx + token.length];
    if (after === undefined || /\s/.test(after)) return true;
    from = idx + token.length;
  }
  return false;
}

/**
 * Folder files that this turn actually uses: @mentions, otherwise the selected
 * file. Empty means send user text through with no snapshot.
 */
export function referencedWorkSources(opts: {
  text: string;
  folder: string;
  files: { path: string; kind: WorkDocKind }[];
  selectedPath: string | null;
}): string[] {
  if (!opts.folder) return [];
  const mentioned: string[] = [];
  for (const file of opts.files) {
    const name = fileBaseName(file.path);
    if (mentionPresent(opts.text, `@${file.path}`) || mentionPresent(opts.text, `@${name}`)) {
      mentioned.push(joinWorkspacePath(opts.folder, file.path));
    }
  }
  if (mentioned.length) return mentioned;
  if (opts.selectedPath && workDocKind(opts.selectedPath)) {
    return [joinWorkspacePath(opts.folder, opts.selectedPath)];
  }
  return [];
}

export function sourceIsWorkImage(path: string): boolean {
  const kind = workDocKind(path);
  return kind != null && isWorkImageKind(kind);
}
