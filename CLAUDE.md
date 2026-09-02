# Agent instructions (WanCode)

Cross-agent review runs directly on GitHub PRs — read `docs/pr-review-protocol.md`
before opening or reviewing any PR. Summary:

- Default roles (swapped by the user 2026-09-01): **Codex implements** (Draft PRs,
  comments prefixed `[codex]`); **Claude Code reviews** (numbered P0/P1/P2
  findings, comments prefixed `[cc]`, one `VERDICT:` line per round). The prefix
  always identifies the actor, never the role. Roles may swap per-PR if stated
  in the PR.
- The four `codex-*` label names are historical and denote the reviewer seat,
  not Codex specifically.
- Whoever executes a merge cannot be that change's reviewer — an independent
  verdict must come from the other agent.
- **`AGENTS.md` and `CLAUDE.md` are mirrors and must be edited together.** Codex
  reads `AGENTS.md`; Claude Code reads `CLAUDE.md`. Updating only one leaves the
  two agents acting on contradictory instructions — this exact drift produced a
  P1 on the very PR that introduced the role swap.
- Review rounds open with exactly `<reviewer-prefix> Reviewed head: <sha>`
  (currently `[cc] Reviewed head: <sha>`); the verdict
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
