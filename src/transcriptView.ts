export type TranscriptView = "minimal" | "standard" | "debug";

export const TRANSCRIPT_VIEW_STORAGE_KEY = "wancode-transcript-view";
export const LEGACY_TRANSCRIPT_VIEW_STORAGE_KEY = "wancode-transcript";
export const TRANSCRIPT_VIEW_ORDER: readonly TranscriptView[] = [
  "standard",
  "minimal",
  "debug",
];

/**
 * Accept the pre-v0.23.3 density values so an upgrade preserves the user's
 * display preference without keeping ambiguous product terminology alive.
 */
export function parseTranscriptView(value: string | null | undefined): TranscriptView {
  switch (value) {
    case "minimal":
    case "compact":
      return "minimal";
    case "debug":
    case "verbose":
      return "debug";
    case "standard":
    case "default":
    case "quiet":
    default:
      return "standard";
  }
}

export function loadTranscriptView(storage: Pick<Storage, "getItem">): TranscriptView {
  return parseTranscriptView(
    storage.getItem(TRANSCRIPT_VIEW_STORAGE_KEY)
      ?? storage.getItem(LEGACY_TRANSCRIPT_VIEW_STORAGE_KEY),
  );
}

