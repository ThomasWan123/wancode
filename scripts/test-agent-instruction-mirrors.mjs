import { readFileSync } from "node:fs";

// Full-file equality is deliberate: AGENTS.md and CLAUDE.md are the same
// repository contract consumed by different agents. If agent-specific guidance
// is ever needed, split shared and agent-specific sections and validate the
// shared source explicitly; do not weaken this gate to substring or fuzzy checks.

const UTF8_BOM = "\uFEFF";

function normalizeNewlines(text) {
  return text.replace(/\r\n?/g, "\n");
}

function assertMirrors(agentsText, claudeText, context) {
  const agentsHasBom = agentsText.startsWith(UTF8_BOM);
  const claudeHasBom = claudeText.startsWith(UTF8_BOM);
  if (agentsHasBom !== claudeHasBom) {
    throw new Error(
      `${context}: AGENTS.md and CLAUDE.md have a UTF-8 BOM mismatch`,
    );
  }

  const agents = normalizeNewlines(agentsText);
  const claude = normalizeNewlines(claudeText);

  if (agents !== claude) {
    const agentsLines = agents.split("\n");
    const claudeLines = claude.split("\n");
    const limit = Math.max(agentsLines.length, claudeLines.length);
    let firstDifference = 0;
    while (
      firstDifference < limit &&
      agentsLines[firstDifference] === claudeLines[firstDifference]
    ) {
      firstDifference += 1;
    }

    throw new Error(
      `${context}: AGENTS.md and CLAUDE.md differ at normalized line ${firstDifference + 1}`,
    );
  }
}

// Positive control: newline style alone is deliberately ignored.
assertMirrors("same\r\ncontent\r\n", "same\ncontent\n", "positive control");

// Negative control: prove the probe detects a real instruction drift.
let negativeControlFailed = false;
try {
  assertMirrors("same\ncontent\n", "same\nchanged\n", "negative control");
} catch {
  negativeControlFailed = true;
}
if (!negativeControlFailed) {
  throw new Error("negative control unexpectedly passed");
}

// Negative control: a BOM mismatch must be diagnosed before newline/content
// comparison so the failure cannot masquerade as a line-1 role drift.
let bomControlFailed = false;
try {
  assertMirrors(`${UTF8_BOM}same\ncontent\n`, "same\ncontent\n", "BOM control");
} catch (error) {
  bomControlFailed = String(error).includes("UTF-8 BOM mismatch");
}
if (!bomControlFailed) {
  throw new Error("BOM mismatch control unexpectedly passed or was misdiagnosed");
}

assertMirrors(
  readFileSync(new URL("../AGENTS.md", import.meta.url), "utf8"),
  readFileSync(new URL("../CLAUDE.md", import.meta.url), "utf8"),
  "repository instruction mirror gate",
);

console.log(
  "AGENTS.md and CLAUDE.md match after CRLF/LF normalization; content-drift and BOM controls passed",
);
