/* Work is conversation-first: the opened project stays in a compact file tree
   beside the thread instead of taking a permanent preview row above it. */
import { useMemo, useState, type DragEvent } from "react";
import { displaySessionTitle } from "../../i18n";
import {
  IconFile,
  IconFolder,
  IconFolderClosed,
  IconPencil,
  IconPlus,
  IconSearch,
  IconTrash,
} from "../../icons";
import { fileBaseName, folderBaseName, pathsFromDataTransfer, type WorkDocKind } from "./workFiles";

export type WorkDeskFile = {
  path: string;
  kind: WorkDocKind;
};

export type WorkDeskSession = {
  session_id: string;
  title: string;
  updated_at: string;
  num_messages: number;
  model_id?: string;
};

type WorkTreeNode = {
  name: string;
  path: string;
  file?: WorkDeskFile;
  children: WorkTreeNode[];
};

function buildWorkTree(files: WorkDeskFile[]): WorkTreeNode[] {
  const root: WorkTreeNode = { name: "", path: "", children: [] };
  for (const file of files) {
    const parts = file.path.replace(/\\/g, "/").split("/").filter(Boolean);
    let parent = root;
    parts.forEach((name, index) => {
      const path = parts.slice(0, index + 1).join("/");
      let child = parent.children.find((entry) => entry.name === name);
      if (!child) {
        child = { name, path, children: [] };
        parent.children.push(child);
      }
      if (index === parts.length - 1) child.file = file;
      parent = child;
    });
  }
  const sort = (nodes: WorkTreeNode[]) => {
    nodes.sort((a, b) => {
      const aFolder = a.children.length > 0 && !a.file;
      const bFolder = b.children.length > 0 && !b.file;
      if (aFolder !== bFolder) return aFolder ? -1 : 1;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
    nodes.forEach((node) => sort(node.children));
  };
  sort(root.children);
  return root.children;
}

function WorkTree(props: {
  nodes: WorkTreeNode[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}) {
  const { nodes, selectedPath, onSelect } = props;
  return (
    <ul className="work-tree">
      {nodes.map((node) =>
        node.file ? (
          <li key={node.path}>
            <button
              type="button"
              className={`work-tree-file ${selectedPath === node.file.path ? "active" : ""}`}
              onClick={() => onSelect(node.file!.path)}
              title={node.file.path}
              aria-label={node.file.path}
            >
              <IconFile size={14} />
              <span className="work-tree-name">{fileBaseName(node.file.path)}</span>
              <span className="work-doc-kind">{node.file.kind.toUpperCase()}</span>
            </button>
          </li>
        ) : (
          <li key={node.path}>
            <details open className="work-tree-folder">
              <summary>
                <IconFolderClosed size={14} />
                <span>{node.name}</span>
              </summary>
              <WorkTree nodes={node.children} selectedPath={selectedPath} onSelect={onSelect} />
            </details>
          </li>
        ),
      )}
    </ul>
  );
}

export function WorkDesk(props: {
  folder: string;
  files: WorkDeskFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
  onNewSession: () => void;
  onOpenFolder: () => void;
  onAddFiles: () => void;
  onDropPaths: (paths: string[]) => void;
  sessions?: WorkDeskSession[];
  searchHits?: WorkDeskSession[] | null;
  sessionSearchQuery?: string;
  activeSessionId?: string;
  starting?: boolean;
  onResumeSession?: (sessionId: string) => void;
  onSearchSessions?: (query: string) => void;
  onRenameSession?: (session: WorkDeskSession, title: string) => void | Promise<void>;
  onDeleteSession?: (session: WorkDeskSession) => void | Promise<void>;
  t: any;
}) {
  const {
    folder,
    files,
    selectedPath,
    onSelect,
    onNewSession,
    onOpenFolder,
    onAddFiles,
    onDropPaths,
    sessions = [],
    searchHits = null,
    sessionSearchQuery = "",
    activeSessionId = "",
    starting = false,
    onResumeSession,
    onSearchSessions,
    onRenameSession,
    onDeleteSession,
    t,
  } = props;
  const [dropActive, setDropActive] = useState(false);
  const [query, setQuery] = useState("");
  const filteredFiles = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return normalized ? files.filter((file) => file.path.toLowerCase().includes(normalized)) : files;
  }, [files, query]);
  const tree = useMemo(() => buildWorkTree(filteredFiles), [filteredFiles]);
  const title = folder ? folderBaseName(folder) : t.workDeskTitle;

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    setDropActive(true);
  }

  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    setDropActive(false);
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    setDropActive(false);
    const paths = pathsFromDataTransfer(e.dataTransfer);
    if (paths.length) onDropPaths(paths);
  }

  return (
    <aside
      className={`work-desk${dropActive ? " drop-active" : ""}`}
      role="region"
      aria-label={t.workDeskTitle}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <button type="button" className="side-new work-new-session" onClick={onNewSession} disabled={starting}>
        <IconPlus size={15} /> {t.sidebarNewSession}
      </button>

      <section className="work-sessions" aria-label={t.sidebarRecent}>
        <div className="work-sessions-head">
          <span>{t.sidebarRecent}</span>
          <IconSearch size={13} />
        </div>
        <input
          className="session-search work-session-search"
          value={sessionSearchQuery}
          placeholder={t.searchPlaceholder}
          aria-label={t.searchPlaceholder}
          onChange={(event) => onSearchSessions?.(event.currentTarget.value)}
        />
        <div className="work-session-list">
          {searchHits !== null && searchHits.length === 0 ? (
            <div className="sidebar-empty">{t.searchNoResults}</div>
          ) : null}
          {searchHits === null && sessions.length === 0 ? (
            <div className="sidebar-empty">{t.noSessions}</div>
          ) : null}
          {(searchHits ?? sessions).map((session) => (
            <div
              key={session.session_id}
              className={`session-item work-session-item ${session.session_id === activeSessionId ? "active" : ""}`}
              title={session.session_id}
              role="button"
              tabIndex={0}
              onClick={() => !starting && onResumeSession?.(session.session_id)}
              onKeyDown={(event) => {
                if (
                  event.target === event.currentTarget &&
                  !starting &&
                  (event.key === "Enter" || event.key === " ")
                ) {
                  event.preventDefault();
                  onResumeSession?.(session.session_id);
                }
              }}
            >
              <div className="session-row">
                <div className="session-title">
                  {displaySessionTitle(session.title, t.untitledSession)}
                </div>
                <div className="session-actions">
                  <button
                    type="button"
                    className="work-session-action"
                    disabled={starting}
                    title={t.renameSession}
                    aria-label={`${t.renameSession}: ${displaySessionTitle(session.title, t.untitledSession)}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      const title = window.prompt(
                        t.renameSession,
                        displaySessionTitle(session.title, t.untitledSession),
                      );
                      if (title?.trim()) void onRenameSession?.(session, title.trim());
                    }}
                  >
                    <IconPencil size={12} />
                  </button>
                  <button
                    type="button"
                    className="work-session-action"
                    disabled={starting}
                    title={t.deleteSession}
                    aria-label={`${t.deleteSession}: ${displaySessionTitle(session.title, t.untitledSession)}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      if (window.confirm(t.deleteConfirm(displaySessionTitle(session.title, t.untitledSession)))) {
                        void onDeleteSession?.(session);
                      }
                    }}
                  >
                    <IconTrash size={12} />
                  </button>
                </div>
              </div>
              <div className="session-meta">
                {session.updated_at.slice(0, 16).replace("T", " ")} · {session.num_messages} {t.messagesUnit}
              </div>
            </div>
          ))}
        </div>
      </section>

      <header className="work-desk-head">
        <div className="work-project-identity" title={folder || t.workDeskEmpty}>
          <IconFolder size={16} />
          <div>
            <div className="work-desk-title">{title}</div>
            <div className="work-desk-sub">{folder ? t.workDeskSubtitle : t.workDeskEmpty}</div>
          </div>
        </div>
        <div className="work-desk-actions">
          <button type="button" className="icon-btn" title={t.workOpenFolder} aria-label={t.workOpenFolder} onClick={onOpenFolder}>
            <IconFolderClosed size={15} />
          </button>
          {folder ? (
            <button type="button" className="icon-btn" title={t.workAddFile} aria-label={t.workAddFile} onClick={onAddFiles}>
              <IconPlus size={15} />
            </button>
          ) : null}
        </div>
      </header>

      {!folder ? (
        <div className="work-desk-empty">
          <IconFolder size={24} />
          <p>{t.workDeskEmpty}</p>
          <p className="work-desk-empty-hint">{t.workDeskEmptyHint}</p>
          <button type="button" className="work-desk-cta" onClick={onOpenFolder}>
            {t.workOpenFolder}
          </button>
        </div>
      ) : (
        <>
          <label className="work-file-search">
            <IconSearch size={13} />
            <input
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              placeholder={t.workSearchFiles}
              aria-label={t.workSearchFiles}
            />
          </label>
          <div className="work-tree-scroll">
            {files.length === 0 ? (
              <div className="work-desk-empty work-desk-dropzone">
                <p>{t.workFolderEmpty}</p>
                <p className="work-desk-empty-hint">{dropActive ? t.workDropActive : t.workDeskEmptyHint}</p>
              </div>
            ) : filteredFiles.length === 0 ? (
              <div className="sidebar-empty">{t.workNoMatchingFiles}</div>
            ) : (
              <WorkTree nodes={tree} selectedPath={selectedPath} onSelect={onSelect} />
            )}
          </div>
          <footer className="work-desk-foot">
            {t.workFilesCount(files.length)} · {t.workDropHint}
          </footer>
        </>
      )}
    </aside>
  );
}
