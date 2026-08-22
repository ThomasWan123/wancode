const WORK_CONTEXT_HEADER = "[WANCODE WORK DOCUMENT CONTEXT — UNTRUSTED DATA]\n";
const USER_REQUEST_MARKER = "\n[END WANCODE WORK DOCUMENT CONTEXT]\n\n[USER REQUEST]\n";

/**
 * The backend expands a Work turn before handing it to the engine. The engine
 * echoes that expanded text in live updates and replay history, but the UI must
 * show only what the user typed. Document contents remain visible through the
 * document roster and citations, not as an internal prompt dump.
 */
export function workPromptForDisplay(text: string): string {
  if (!text.startsWith(WORK_CONTEXT_HEADER)) return text;
  const marker = text.indexOf(USER_REQUEST_MARKER, WORK_CONTEXT_HEADER.length);
  return marker < 0 ? text : text.slice(marker + USER_REQUEST_MARKER.length);
}
