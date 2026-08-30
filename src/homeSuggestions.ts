import type { SurfaceKind } from "./surface";

export type HomeSuggestion = { label: string; prompt: string };

/**
 * Home suggestions are surface language, not generic repository shortcuts.
 * Chat and Work deliberately ignore git/test state; Code keeps the existing
 * workspace-aware suggestions.
 */
export function buildSuggestions(
  files: string[],
  git: any,
  t: any,
  surface: SurfaceKind,
): HomeSuggestion[] {
  if (surface === "chat") {
    return [
      { label: t.sugChatAsk, prompt: t.sugChatAskP },
      { label: t.sugChatExplain, prompt: t.sugChatExplainP },
      { label: t.sugChatSummarize, prompt: t.sugChatSummarizeP },
    ];
  }

  if (surface === "work") {
    return [
      { label: t.sugWorkSummarize, prompt: t.sugWorkSummarizeP },
      { label: t.sugWorkFind, prompt: t.sugWorkFindP },
      { label: t.sugWorkCompare, prompt: t.sugWorkCompareP },
    ];
  }

  const out: HomeSuggestion[] = [];
  const lower = files.map((file) => file.toLowerCase());
  const hasReadme = lower.some((file) => file === "readme.md" || file.endsWith("/readme.md"));
  const hasTests = lower.some(
    (file) => /(^|\/)(tests?|__tests__|spec)\//.test(file) || /\.(test|spec)\.[a-z]+$/.test(file),
  );
  const dirty = git?.isRepo ? (git.files?.length ?? 0) : 0;

  if (dirty > 0) {
    out.push({ label: t.sugReviewChanges, prompt: t.sugReviewChangesP });
    out.push({ label: t.sugCommitMsg, prompt: t.sugCommitMsgP });
  }
  if (hasReadme) out.push({ label: t.sugSummarize, prompt: t.sugSummarizeP });
  if (hasTests) out.push({ label: t.sugRunTests, prompt: t.sugRunTestsP });
  out.push({ label: t.sugExplainStruct, prompt: t.sugExplainStructP });
  out.push({ label: t.sugFindBugs, prompt: t.sugFindBugsP });
  return out.slice(0, 4);
}
