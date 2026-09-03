import { readFileSync } from "node:fs";

function normalizeNewlines(text) {
  return text.replace(/\r\n?/g, "\n");
}

function assertMirrors(agentsText, claudeText, context) {
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

assertMirrors(
  readFileSync(new URL("../AGENTS.md", import.meta.url), "utf8"),
  readFileSync(new URL("../CLAUDE.md", import.meta.url), "utf8"),
  "repository instruction mirror gate",
);

console.log(
  "AGENTS.md and CLAUDE.md match after CRLF/LF normalization; positive and negative controls passed",
);
