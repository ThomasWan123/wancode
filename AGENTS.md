# Agent instructions (WanCode)

Cross-agent review runs directly on GitHub PRs — read `docs/pr-review-protocol.md`
before opening or reviewing any PR. Summary:

- Default roles: Claude Code implements (Draft PRs, comments prefixed `[cc]`);
  Codex reviews (numbered P0/P1/P2 findings, comments prefixed `[codex]`, one
  `VERDICT:` line per round). Roles may swap per-PR if stated in the PR.
- Review rounds open with exactly `[codex] Reviewed head: <sha>`; the verdict
  binds to that SHA. Final `VERDICT: ACCEPT` requires all required checks green
  on that exact head. P0/P1 findings issue `VERDICT: BLOCK` immediately — they
  never wait for CI; only a no-blocker round waiting solely on checks ends with
  `PRELIMINARY — no verdict`. Any push after ACCEPT invalidates the verdict and
  any merge authorization; the label reverts to `needs-codex-review`. The four
  workflow labels are mutually exclusive.
- Verify every finding independently before accepting it; reply per-finding.
- Never merge, tag, release, or change scope without explicit user authorization.
- After 3 non-converging rounds on one finding: label `needs-user-decision`, stop.
- Credentials never appear in PRs, comments, or chat.
- PR bodies use the template: evidence table, NOT-RUN made explicit, claims bound
  to test names / CI runs / commits.
