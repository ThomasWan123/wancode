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

**Current default (set by the user on 2026-09-01): Codex implements, CC reviews.**
This reverses the original assignment; the rules below are role-based and apply
unchanged either way.

| Actor | Role (current default) | Writes |
|---|---|---|
| **Codex** | Implementer. Opens Draft PRs, responds to findings, pushes fixes. | Branches, commits, PR body, comments prefixed `[codex]` |
| **CC** (Claude Code) | Reviewer. Reviews every Draft PR labeled `needs-codex-review`. | PR review comments prefixed `[cc]` |
| **User** | Arbiter. Intervenes only at the authorization points below. | Merge/release approvals, deadlock rulings |

**The comment prefix always identifies the actor, never the role.** Both agents
post through the same GitHub account, so the prefix is the *only* way a later
reader can tell who wrote a line. `[cc]` is always Claude Code and `[codex]` is
always Codex, whichever seat they hold. A review round therefore opens with
`<actor-prefix> Reviewed head: <sha>` — under the current default that is
`[cc] Reviewed head: <sha>`.

**The four label names are historical.** `needs-codex-review` / `codex-accepted` /
`codex-blocked` denote the **reviewer seat**, not Codex specifically. They were
left unrenamed on purpose: no automation reads them (verified — no reference in
`.github/` or `scripts/`), and renaming would rewrite the meaning of every past
PR's label history for no functional gain.

Roles may still swap per-PR — the opening comment states who holds which role
for that PR when it differs from the default above.

**A merge executor cannot be that change's reviewer.** Whoever pushed the merge
button has a stake in the outcome; an independent verdict must come from the
other agent. This is not about which seat is assigned — it is about not being
both the actor and the auditor of the same act.

Both agents operate through the same GitHub account, so GitHub's formal
Approve/Request-changes cannot distinguish them and self-approval is blocked.
Identity and verdicts are therefore carried by **comment conventions and labels**,
which are authoritative under this protocol.

## Flow

1. **Implementer** opens a **Draft PR** using the PR template (evidence table
   mandatory), adds label `needs-codex-review`, and posts
   `<implementer-prefix> READY FOR REVIEW` when CI is green (or explains why
   review should start before green). Under the current default that is
   `[codex] READY FOR REVIEW`.
2. **Reviewer** posts one complete review comment per round:
   - First line is exactly `<reviewer-prefix> Reviewed head: <sha>` — the
     **actor's** prefix, then the reviewed head SHA. Under the current default
     that is `[cc] Reviewed head: <sha>`. The verdict binds to that SHA and to
     nothing else.
   - Findings numbered and severity-tagged **P0 / P1 / P2**.
   - Each finding names file/line or test, states the failure scenario, and where
     possible how to verify.
   - Ends with exactly one verdict line:
     `VERDICT: ACCEPT` | `VERDICT: BLOCK (P0=n, P1=n)` | `VERDICT: NEEDS-USER (reason)`.
   - **A final `VERDICT: ACCEPT` requires all required checks green on the exact
     reviewed head.** Reviewing before checks finish is allowed and encouraged,
     and splits two ways: a round that finds any **P0/P1 issues
     `VERDICT: BLOCK` immediately** — blocking findings never wait for CI. Only
     a review with no blocking findings, whose sole remaining condition is
     pending checks, ends with
     `PRELIMINARY — no verdict (checks pending on <sha>)` and carries no label
     transition.
3. **Implementer** independently verifies every finding before accepting it
   (verify-then-agree — never adopt a finding unchecked; both agents have been
   wrong). Replies per-finding: `confirmed + fix` / `refuted + evidence` /
   `needs-user`. Pushes fixes, updates the evidence table, re-posts
   `<implementer-prefix> READY FOR REVIEW`.
4. Repeat. On `VERDICT: ACCEPT`, reviewer swaps the label to `codex-accepted`;
   implementer flips the PR to Ready and posts
   `<implementer-prefix> REQUESTING MERGE AUTHORIZATION` with the final evidence
   summary and the accepted head SHA.
5. **User** authorizes merge (a PR comment `批准合并` / `approve merge`, or via
   chat). Implementer merges (squash by default), deletes the branch.

### Staleness: verdicts die with the head they reviewed

Any push after a `READY FOR REVIEW` invalidates the round in progress; any push
after `VERDICT: ACCEPT` — including typo-only follow-ups — invalidates that
verdict **and any merge authorization already granted on it**. Whoever pushes
removes `codex-accepted`, restores `needs-codex-review`, and the new head goes
through a fresh round (which may be short, but its changed bytes get reviewed
and its checks must pass). A merge may only ever execute the exact SHA the user
authorized.

### Label state machine

The four labels are **mutually exclusive** — at most one on a PR at any time.

| Transition | Trigger | Who |
|---|---|---|
| — → `needs-codex-review` | Draft PR opened / `READY FOR REVIEW` posted | implementer |
| `needs-codex-review` → `codex-blocked` | `VERDICT: BLOCK` | reviewer |
| `codex-blocked` → `needs-codex-review` | fixes pushed + new `READY FOR REVIEW` | implementer |
| `needs-codex-review` → `codex-accepted` | `VERDICT: ACCEPT` (checks green on head) | reviewer |
| `codex-accepted` → `needs-codex-review` | any push after ACCEPT | whoever pushes |
| any → `needs-user-decision` | deadlock / scope question | either agent |

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
