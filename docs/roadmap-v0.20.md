# v0.20 Direction — pragmatic plan after the v0.19.1 reissue

Status: PROPOSED (cc draft, awaiting codex review). Horizon: ~4–6 weeks.
Decision owner: user. Execution: task split in §4.

## 0. Where we stand (2026-08-10)

Shipped and verified: v0.19.1 public — an **approved honest-unsigned release**
(all five checklist gates + upgrade E2E passed; Authenticode explicitly open as
QA-019-007),
Chat/Code layered surfaces with local-extension hard isolation, model-identity
hardening (catalog-key routing end to end), professional QA system (PR #29 — still Draft, its scorecard bound to the
withdrawn v0.19.0 candidate and reading NO-GO, so it must be reconciled to the
v0.19.1 evidence before merge), Windows release-candidate bundle gate
(PR #31 — merged 2026-08-10), audit-mode restore (PR #35 — merged), GitHub-native CC⇄Codex review protocol (merged, already caught real defects in
its own first three uses).

Open debts: Authenticode signing (QA-019-007), competitive benchmarks NOT-RUN,
reused-target collision (QA-019-002), engine warnings (QA-019-004), ~35-minute
rust CI wall time per round, single-mirror update path (gh-proxy SPOF, one
recorded zero-byte incident).

Strategic goal set by the user: **be better than ChatGPT (Codex) and Claude
Code** — which we interpret as: measurably better on tasks our users run,
frictionless to install and update, and differentiated where the competitors
structurally cannot follow.

## 1. Track A — Trust & distribution (remove install/update friction)

| # | Item | Why | Exit criterion |
|---|---|---|---|
| A1 | Code signing | **DEFERRED by user scope ruling (2026-08-11): WanCode is currently a personal-use product.** Authenticode is a distribution-trust signal, not an operator security control; the update chain is already minisign-verified independently. Cost/friction (annual fee, identity validation, per-release OTP) buys ~nothing at personal scale. Note: Azure Trusted Signing individuals are US/CA-only, so the live option when triggered is a Certum-style individual cloud cert | Reactivation triggers (any one): public promotion push, a real external user base, marketplace/store distribution, or strict-EDR environments. Until then releases follow the v0.19.1 honest-unsigned pattern (explicit disclosure + SHA-256 + minisign chain). `require_authenticode` stays available behind workflow_dispatch for the day it flips |
| A2 | Merge QA system (PR #29) + bundle gate (PR #31) | Both are finished evidence institutions sitting in Draft | Merged; scorecard becomes the standing release template |
| A3 | Multi-mirror updater failover | gh-proxy is a single point of failure with a recorded zero-byte incident; retry ≠ failover | Mirrors are **transport-only, never trust roots**: the same minisign-signed manifest and artifact identity must verify under the pinned updater key regardless of source; signature/hash/version mismatch fails closed and advances to the next source without executing bytes; downloaded bytes are re-verified before launch. Ordered mirror list with origin fallback; failure surfaced in UI. E2E covers positive origin/mirror controls plus zero-byte, truncation, corruption, stale-manifest, timeout, mismatched-signature, and origin-fallback cases |

## 2. Track B — Prove "better" (measurement, not claims)

| # | Item | Why | Exit criterion |
|---|---|---|---|
| B1 | Competitive calibration run | The QA plan defines it; it has never run. "Better than X" stays a claim until measured | **Step 1: a committed benchmark protocol** (codex-reviewed before any run): frozen repo snapshots + task specs; disclosed product/model/version/settings; equivalent permissions and tool access; intervention-counting rules; multiple trials or stated uncertainty; raw transcripts archived; cost normalization; failure adjudication independent of the product under test; NOT-RUN/missing access can never score as a win. **Step 2:** run it on WanCode / Claude Code / Codex; publish favorable and unfavorable results alike in docs/evidence; repeat per release |
| B2 | Public provider compatibility matrix | Our compliance suite (4a/4b) is evidence nobody else publishes; converts "OpenAI-compatible" from slogan to contract | docs page generated from CI compliance summaries; per-provider rows with evidence links; linked from README |

## 3. Track C — Differentiate (where competitors can't follow)

| # | Item | Why | Exit criterion |
|---|---|---|---|
| C1 | Windows AI-process governance phase 2 | Job-Object tree control shipped; ETW file-write audit PoC validated. As of 2026-08 our dated comparison matrix (docs/roadmap-vs-claude-code-codex.md) records no equivalent in-app auditable process-tree file-write governance on Windows in either competitor; the claim is scoped to that capability, dated, and must be re-verified before any marketing use | ETW-based write audit of the AI process tree behind a feature flag; audit log viewable in-app; design doc first, codex-reviewed; implementation by cc after design ACCEPT |
| C2 | Reasoning-effort selector | Both competitors have it; we don't; engine exposes it | Per-model effort selector where the provider supports it; capability-gated (unknown ≠ advertised) |
| C3 | Memory edit/refresh wiring | `memory/flush`, `memory/rewrite` engine methods exist unwired | Settings surface for project memory; engine round-trip test |
| C4 | Dogfooding cadence | The best defect source we have; every fielded bug this cycle came from real use | Fixed weekly hours using WanCode on real work; defects → ledger → priority by frequency×severity |

## 4. Task split

| Owner | Items | Notes |
|---|---|---|
| **cc** (implementer default) | A3, B1 harness + run, B2 generator, C2, C3, CI lane split (§5) | Draft PRs per protocol |
| **codex** (reviewer default + QA owner) | Reviews of all of the above; A2 finalization (owns PR #29/#31); B1 scorecard adjudication; C1 design review before any code | Verdicts per protocol |
| **user** | A1 reactivation decision (deferred; see triggers); competitor accounts for B1; merge/release authorizations | A1 left the critical path per the 2026-08-11 personal-use scope ruling |

Debt assignments (no orphans): QA-019-002 reused-target collision — owner cc,
Week 2, exit = unique lib/integration artifacts + a reused-target regression
check in CI. QA-019-004 engine warnings — explicitly deferred, owner codex
(QA backlog), trigger = next Rust toolchain upgrade; revisit date 2026-09-15.
C1 implementer after the codex-reviewed design: cc.

Role swaps per-PR remain allowed and stated in the PR.

## 5. Process guardrails (keep, plus one change)

- PR review protocol, evidence tables, NOT-RUN discipline, mutation testing
  where assertions need teeth: unchanged.
- **CI lane split** (new, guarded): prerequisite is a **required pre-merge
  gate** — a branch ruleset on `main` requiring PRs and green checks on the
  exact head (none existed as of 2026-08-11; being created as part of this
  plan). Then: fast lane (wancode lib + frontend + clippy + migration-audit,
  target < 10 min) required on every PR; the full engine battery (routing,
  compaction, model-identity, canary, Chat fan-out) stays **required before
  merge** — routed by a path-impact matrix whose fallback is fail-safe: any
  engine/vendor/build/CI/security-touching change, or any unmatched path,
  runs the full battery. The full battery remains required for every merge
  until the routing itself has mutation/negative evidence. Nightly runs may
  duplicate the full lane as drift detection; they never replace a merge
  gate. A ~35-minute round-trip on every push is the single largest measured
  drag, but speed never buys removal of a safety gate.

## 6. Sequence (indicative)

- **Week 1**: reconcile + merge #29 (#31/#35 already merged); CI lane split;
  A3 design.
- **Weeks 2–3**: B1 first full run + scorecard; A3 implementation; C2.
- **Weeks 3–6**: C1 design (codex-reviewed) → implementation (cc); B2 page;
  C3; v0.20 release follows the honest-unsigned pattern while A1 stays
  deferred (user scope ruling; see A1 triggers).

## 7. Explicitly not doing (and why)

- Plugin marketplace — no ecosystem to serve yet; revisit on demand signal.
- Cloud execution/delegation — depends on infrastructure we don't control.
- Session sharing — gated on upstream login state we can never obtain.
