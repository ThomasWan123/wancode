/** Popup state for @-mentions and slash commands in the composer. */
export type ComposerPopup = {
  kind: "slash" | "at";
  query: string;
  sel: number;
};

/**
 * A popup only blocks Send/Enter when a row is actually on screen.
 * Hidden or empty popups (slash with zero matches, stale @ with no files)
 * must not swallow the key.
 */
export function popupIsVisible(
  popup: ComposerPopup | null | undefined,
  itemCount: number,
): boolean {
  return !!popup && itemCount > 0;
}

/** Detect an in-progress slash or @ token at `caret`. Spaces never match. */
export function detectComposerPopup(
  value: string,
  caret: number,
): Omit<ComposerPopup, "sel"> | null {
  const before = value.slice(0, caret);
  if (/^\/[\w-]*$/.test(before) && value === before) {
    return { kind: "slash", query: before };
  }
  const m = before.match(/(?:^|\s)@([^\s@]*)$/);
  if (m) return { kind: "at", query: m[1] };
  return null;
}
