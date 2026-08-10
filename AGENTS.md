# Agent instructions (WanCode)

Cross-agent review runs directly on GitHub PRs — read `docs/pr-review-protocol.md`
before opening or reviewing any PR. Summary:

- Default roles: Claude Code implements (Draft PRs, comments prefixed `[cc]`);
  Codex reviews (numbered P0/P1/P2 findings, comments prefixed `[codex]`, one
  `VERDICT:` line per round). Roles may swap per-PR if stated in the PR.
- Verify every finding independently before accepting it; reply per-finding.
- Never merge, tag, release, or change scope without explicit user authorization.
- After 3 non-converging rounds on one finding: label `needs-user-decision`, stop.
- Credentials never appear in PRs, comments, or chat.
- PR bodies use the template: evidence table, NOT-RUN made explicit, claims bound
  to test names / CI runs / commits.
