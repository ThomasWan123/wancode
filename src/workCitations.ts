export type WorkCitationSource = {
  documentName: string;
  blockPath: string;
};

export type WorkCitationCheck = {
  citation: string;
  documentName: string;
  blockPath: string;
  status: "verified" | "unverifiable";
  reason?: "missing" | "ambiguous";
};

// This grammar mirrors the four block-path families emitted by the Rust
// parsers. Requiring a known path shape avoids treating ordinary prose such as
// "[2025 — 2026]" as a failed citation.
const BLOCK_PATH = String.raw`(?:body\/p\[\d+\]|page\[\d+\]\/chunk\[\d+\]|workbook\/sheet\[[^\]\r\n]+\]\/cell\[[^\]\r\n]+\]|slides\/slide\[\d+\]\/text\[\d+\])`;
const CITATION = new RegExp(String.raw`\[([^\r\n]+?)\s+—\s+(${BLOCK_PATH})\]`, "gu");

/**
 * Verify every model-emitted Work citation against the exact prompt catalog.
 * A pair must occur exactly once: zero is invented/stale, more than one is
 * ambiguous. We never guess which duplicate document the model meant.
 */
export function verifyWorkCitations(
  text: string,
  sources: readonly WorkCitationSource[],
): WorkCitationCheck[] {
  const checks: WorkCitationCheck[] = [];
  for (const match of text.matchAll(CITATION)) {
    const citation = match[0];
    const documentName = match[1].trim();
    const blockPath = match[2].trim();
    const count = sources.filter(
      (source) => source.documentName === documentName && source.blockPath === blockPath,
    ).length;
    checks.push({
      citation,
      documentName,
      blockPath,
      status: count === 1 ? "verified" : "unverifiable",
      ...(count === 0 ? { reason: "missing" as const } : count > 1 ? { reason: "ambiguous" as const } : {}),
    });
  }
  return checks;
}

export function attachWorkCitationChecks<T extends {
  kind: string;
  text?: string;
  citationChecks?: WorkCitationCheck[];
}>(items: readonly T[], sources: readonly WorkCitationSource[]): T[] | readonly T[] {
  let turnStart = -1;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (items[index].kind === "user") {
      turnStart = index;
      break;
    }
  }
  // A completion catalog is valid only for a turn whose user boundary is
  // present in this transcript. Never apply today's document snapshot to
  // assistant messages loaded from an older or incomplete replay.
  if (turnStart < 0) return items;

  let next: T[] | undefined;
  for (let index = turnStart + 1; index < items.length; index += 1) {
    const item = items[index];
    if (item.kind !== "assistant" || item.citationChecks !== undefined) continue;
    next ??= items.slice();
    next[index] = {
      ...item,
      citationChecks: verifyWorkCitations(item.text ?? "", sources),
    };
  }
  return next ?? items;
}
