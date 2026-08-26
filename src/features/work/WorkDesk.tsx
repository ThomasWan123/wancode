/* Work surface: talk about documents that live on disk in an opened folder.
   Preview is identity (and extractable text if a caller already has it) —
   not a fake PDF/Excel editor and not a sha256 fingerprint panel. */
import { useState, type DragEvent } from "react";
import { IconFile, IconFolder } from "../../icons";
import { fileBaseName, folderBaseName, pathsFromDataTransfer, type WorkDocKind } from "./workFiles";

export type WorkDeskFile = {
  path: string;
  kind: WorkDocKind;
};

export function WorkDesk(props: {
  folder: string;
  files: WorkDeskFile[];
  selectedPath: string | null;
  extractText?: string | null;
  onSelect: (path: string) => void;
  onOpenFolder: () => void;
  onAddFiles: () => void;
  onDropPaths: (paths: string[]) => void;
  t: any;
}) {
  const {
    folder,
    files,
    selectedPath,
    extractText,
    onSelect,
    onOpenFolder,
    onAddFiles,
    onDropPaths,
    t,
  } = props;
  const [dropActive, setDropActive] = useState(false);
  const selected = files.find((f) => f.path === selectedPath) ?? null;
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
    <section
      className={`work-desk${dropActive ? " drop-active" : ""}`}
      role="region"
      aria-label={t.workDeskTitle}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <header className="work-desk-head">
        <div>
          <div className="work-desk-title">{title}</div>
          <div className="work-desk-sub">
            {folder ? t.workDeskSubtitle : t.workDeskEmpty}
          </div>
        </div>
        <div className="work-desk-actions">
          <button type="button" onClick={onOpenFolder}>
            {t.workOpenFolder}
          </button>
          {folder ? (
            <button type="button" className="ghost" onClick={onAddFiles}>
              {t.workAddFile}
            </button>
          ) : null}
        </div>
      </header>
      {!folder ? (
        <div className="work-desk-empty">
          <IconFolder size={28} />
          <p>{t.workDeskEmpty}</p>
          <p className="work-desk-empty-hint">{t.workDeskEmptyHint}</p>
          <button type="button" className="work-desk-cta" onClick={onOpenFolder}>
            {t.workOpenFolder}
          </button>
        </div>
      ) : files.length === 0 ? (
        <div className="work-desk-empty work-desk-dropzone">
          <IconFolder size={28} />
          <p>{t.workFolderEmpty}</p>
          <p className="work-desk-empty-hint">{dropActive ? t.workDropActive : t.workDeskEmptyHint}</p>
        </div>
      ) : (
        <div className="work-desk-split">
          <ul className="work-desk-list">
            {files.map((f) => (
              <li key={f.path}>
                <button
                  type="button"
                  className={`work-desk-card ${selectedPath === f.path ? "active" : ""}`}
                  onClick={() => onSelect(f.path)}
                >
                  <IconFile size={16} />
                  <span className="work-doc-kind">{f.kind.toUpperCase()}</span>
                  <span className="work-doc-name">{fileBaseName(f.path)}</span>
                </button>
              </li>
            ))}
          </ul>
          <div className="work-desk-preview">
            {selected ? (
              <>
                <h3>{fileBaseName(selected.path)}</h3>
                <p className="work-desk-meta">
                  {selected.kind.toUpperCase()}
                  {selected.path !== fileBaseName(selected.path) ? ` · ${selected.path}` : ""}
                </p>
                {extractText ? <pre className="work-desk-extract">{extractText}</pre> : null}
              </>
            ) : (
              <p className="work-desk-preview-hint">{t.workSelectHint}</p>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
