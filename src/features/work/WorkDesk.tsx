/* Work surface as a document desk: visible list, empty copy, metadata preview.
   Binary PDF/DOCX page preview needs a staged absolute path the frontend does
   not have without a new backend command — show identity, not a fake editor. */
import { IconFile, IconFolder } from "../../icons";

export type WorkDoc = {
  import_id: string;
  display_name: string;
  kind: string;
  source_sha256: string;
};

export function WorkDesk(props: {
  docs: WorkDoc[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onImport: () => void;
  t: any;
}) {
  const { docs, selectedId, onSelect, onImport, t } = props;
  const selected = docs.find((d) => d.import_id === selectedId) ?? null;

  return (
    <section className="work-desk" role="region" aria-label={t.workDeskTitle}>
      <header className="work-desk-head">
        <div>
          <div className="work-desk-title">{t.workDeskTitle}</div>
          <div className="work-desk-sub">{t.workDeskSubtitle}</div>
        </div>
        <button onClick={onImport}>{t.workImport}</button>
      </header>
      {docs.length === 0 ? (
        <div className="work-desk-empty">
          <IconFolder size={28} />
          <p>{t.workDeskEmpty}</p>
          <p className="work-desk-empty-hint">{t.workDeskEmptyHint}</p>
        </div>
      ) : (
        <div className="work-desk-split">
          <ul className="work-desk-list">
            {docs.map((d) => (
              <li key={d.import_id}>
                <button
                  className={`work-desk-card ${selectedId === d.import_id ? "active" : ""}`}
                  onClick={() => onSelect(d.import_id)}
                >
                  <IconFile size={16} />
                  <span className="work-doc-kind">{d.kind.toUpperCase()}</span>
                  <span className="work-doc-name">{d.display_name}</span>
                </button>
              </li>
            ))}
          </ul>
          <div className="work-desk-preview">
            {selected ? (
              <>
                <h3>{selected.display_name}</h3>
                <p className="work-desk-meta">
                  {selected.kind.toUpperCase()} · {t.workFingerprint} {selected.source_sha256.slice(0, 12)}…
                </p>
                <p className="work-desk-preview-hint">{t.workPreviewHint}</p>
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
