# PR Review Protocol (CC ⇄ Codex, direct on GitHub)

Status: active since 2026-08-10. Supersedes the manual copy-paste relay between agents.

## Why

During v0.18.6–v0.19.0 the cross-review loop between Claude Code (implementer) and
Codex (reviewer) caught 15+ real bugs neither agent found alone — but every message
was relayed by hand through the user. That made the user the bus and the bottleneck,
truncated review feedback in transit, and capped iteration speed at human copy-paste
frequency. This protocol moves the loop onto GitHub PRs, where both agents read and
write directly. The user steps back from bus to arbiter.

## Roles

| Actor | Role | Writes |
|---|---|---|
| **CC** (Claude Code) | Implementer. Opens Draft PRs, responds to findings, pushes fixes. | Branches, commits, PR body, comments prefixed `[cc]` |
| **Codex** | Reviewer. Reviews every Draft PR labeled `needs-codex-review`. | PR review comments prefixed `[codex]` |
| **User** | Arbiter. Intervenes only at the authorization points below. | Merge/release approvals, deadlock rulings |

Roles may swap per-PR (Codex implements, CC reviews) — the opening comment states
who holds which role for that PR. The same rules apply either way.

Both agents operate through the same GitHub account, so GitHub's formal
Approve/Request-changes cannot distinguish them and self-approval is blocked.
Identity and verdicts are therefore carried by **comment conventions and labels**,
which are authoritative under this protocol.

## Flow

1. **Implementer** opens a **Draft PR** using the PR template (evidence table
   mandatory), adds label `needs-codex-review`, and posts `[cc] READY FOR REVIEW`
   when CI is green (or explains why review should start before green).
2. **Reviewer** posts one complete review comment per round:
   - Prefix `[codex]`, findings numbered and severity-tagged **P0 / P1 / P2**.
   - Each finding names file/line or test, states the failure scenario, and where
     possible how to verify.
   - Ends with exactly one verdict line:
     `VERDICT: ACCEPT` | `VERDICT: BLOCK (P0=n, P1=n)` | `VERDICT: NEEDS-USER (reason)`.
3. **Implementer** independently verifies every finding before accepting it
   (verify-then-agree — never adopt a finding unchecked; both agents have been
   wrong). Replies per-finding: `confirmed + fix` / `refuted + evidence` /
   `needs-user`. Pushes fixes, updates the evidence table, re-posts
   `[cc] READY FOR REVIEW`.
4. Repeat. On `VERDICT: ACCEPT`, reviewer swaps the label to `codex-accepted`;
   implementer flips the PR to Ready and posts `[cc] REQUESTING MERGE AUTHORIZATION`
   with the final evidence summary.
5. **User** authorizes merge (a PR comment `批准合并` / `approve merge`, or via
   chat). Implementer merges (squash by default), deletes the branch.

## Authorization points (user only — agents must stop and ask)

- Merging any PR into `main`.
- Anything in the release pipeline: version bumps, tags, GitHub releases,
  asset uploads, `latest.json`.
- Scope changes: adding work a PR was not opened for, or abandoning a gate.
- Deadlock: after **3 rounds** on the same finding without convergence, either
  agent posts `VERDICT: NEEDS-USER` with both positions summarized in ≤10 lines
  each, applies label `needs-user-decision`, and stops.
- Anything involving credentials, payments, or publishing outside this repo.
  Credentials never appear in PRs, comments, or chat — keyring only.

## Review discipline (lessons already paid for)

These rules encode failures that actually happened in v0.18.x–v0.19; they are not
aspirational.

1. **Claims are bound to evidence.** Every "done/green/fixed" in a PR body links a
   test name, CI run, or commit. Anything not run is listed as `NOT-RUN` — never
   omitted. Documentation claims score nothing.
2. **Reverse-direction check.** Every fix states what the opposite path is and how
   it is protected (the recurring blind spot: five consecutive rounds of
   fixing one direction while breaking the other).
3. **Pure-function green ≠ transaction green.** Identity/state writes require a
   "multiple writes, then read back the final persisted state" assertion.
4. **Negative assertions need a positive control.** A "nothing leaked/zero calls"
   assertion counts only next to a positive case proving the probe can detect
   the thing at all.
5. **New regression tests prove they can fail** (RED-first or mutation check)
   before they count as coverage.
6. **Full findings, not summaries.** Reviewer posts complete numbered findings in
   the PR — never "as discussed elsewhere". The transit-truncation failure mode
   (implementer guessing what a half-pasted review meant) must not recur.
7. **Overclaim is a finding.** A claim later shown false ("zero side effects",
   "all four gates closed") is itself P1 — state narrowing is cheaper than
   retraction.

## Labels

| Label | Meaning |
|---|---|
| `needs-codex-review` | Draft PR awaiting reviewer round |
| `codex-accepted` | Reviewer verdict ACCEPT; awaiting user merge authorization |
| `codex-blocked` | Open P0/P1 findings; implementer working |
| `needs-user-decision` | Deadlock or scope question; both agents stopped |

## Out of scope

This protocol governs the review loop only. Release gates, QA scoring, and the
NO-GO discipline live in the professional QA system (PR #29) and are unchanged.
