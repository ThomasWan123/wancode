import { IconCheck, IconShield } from "../../icons";

export default function CitationChecks({ checks, t }: { checks: any[]; t: any }) {
  return (
    <div className="citation-checks" aria-label={t.citationChecksLabel}>
      {checks.map((check, index) => (
        <span
          key={`${check.citation}-${index}`}
          className={`citation-check ${check.status}`}
          title={check.status === "verified" ? t.citationVerified : t.citationUnverifiable}
        >
          {check.status === "verified" ? <IconCheck size={12} /> : <IconShield size={12} />}
          <span>{check.documentName} — {check.blockPath}</span>
          <strong>{check.status === "verified" ? t.citationVerified : t.citationUnverifiable}</strong>
        </span>
      ))}
    </div>
  );
}
