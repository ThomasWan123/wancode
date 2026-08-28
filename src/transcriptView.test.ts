import { describe, expect, it } from "vitest";
import {
  LEGACY_TRANSCRIPT_VIEW_STORAGE_KEY,
  loadTranscriptView,
  parseTranscriptView,
  TRANSCRIPT_VIEW_STORAGE_KEY,
} from "./transcriptView";

describe("transcript view preference", () => {
  it.each([
    ["compact", "minimal"],
    ["default", "standard"],
    ["quiet", "standard"],
    ["verbose", "debug"],
    ["minimal", "minimal"],
    ["standard", "standard"],
    ["debug", "debug"],
    ["future-value", "standard"],
    [null, "standard"],
  ])("maps %s to %s", (raw, expected) => {
    expect(parseTranscriptView(raw)).toBe(expected);
  });

  it("prefers the new key and falls back to the legacy key", () => {
    const values = new Map([
      [TRANSCRIPT_VIEW_STORAGE_KEY, "debug"],
      [LEGACY_TRANSCRIPT_VIEW_STORAGE_KEY, "compact"],
    ]);
    expect(loadTranscriptView({ getItem: (key) => values.get(key) ?? null })).toBe("debug");

    values.delete(TRANSCRIPT_VIEW_STORAGE_KEY);
    expect(loadTranscriptView({ getItem: (key) => values.get(key) ?? null })).toBe("minimal");
  });
});
