# v0.20 Direction — pragmatic plan after the v0.19.1 reissue

Status: PROPOSED (cc draft, awaiting codex review). Horizon: ~4–6 weeks.
Decision owner: user. Execution: task split in §4.

## 0. Where we stand (2026-08-10)

Shipped and verified: v0.19.1 public (honest-unsigned, full gate evidence),
Chat/Code layered surfaces with local-extension hard isolation, model-identity
hardening (catalog-key routing end to end), professional QA system (PR #29,
pending merge), Windows release-candidate bundle gate (PR #31, in flight),
GitHub-native CC⇄Codex review protocol (merged, already caught real defects in
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
| A1 | Code signing | Unsigned = SmartScreen wall for every new user; the one gate v0.19.1 shipped without | User obtains cert (Azure Trusted Signing or OV); release pipeline signs exe+msi+nsis; `require_authenticode` gate flipped on; first signed release |
| A2 | Merge QA system (PR #29) + bundle gate (PR #31) | Both are finished evidence institutions sitting in Draft | Merged; scorecard becomes the standing release template |
| A3 | Multi-mirror updater failover | gh-proxy is a single point of failure with a recorded zero-byte incident; retry ≠ failover | Updater tries ordered mirror list, falls back to origin; failure surfaced in UI; covered by update E2E |

## 2. Track B — Prove "better" (measurement, not claims)

| # | Item | Why | Exit criterion |
|---|---|---|---|
| B1 | Competitive calibration run | The QA plan defines it; it has never run. "Better than X" stays a claim until measured | Same task set (real-repo bugfix, multi-file refactor, long-session resume, provider switch) on WanCode / Claude Code / Codex; scorecard (success, wall time, token cost, interventions) in docs/evidence; repeat per release |
| B2 | Public provider compatibility matrix | Our compliance suite (4a/4b) is evidence nobody else publishes; converts "OpenAI-compatible" from slogan to contract | docs page generated from CI compliance summaries; per-provider rows with evidence links; linked from README |

## 3. Track C — Differentiate (where competitors can't follow)

| # | Item | Why | Exit criterion |
|---|---|---|---|
| C1 | Windows AI-process governance phase 2 | Job-Object tree control shipped; ETW file-write audit PoC validated. Neither Claude Code nor Codex sandboxes on Windows at all | ETW-based write audit of the AI process tree behind a feature flag; audit log viewable in-app; design doc first, codex-reviewed |
| C2 | Reasoning-effort selector | Both competitors have it; we don't; engine exposes it | Per-model effort selector where the provider supports it; capability-gated (unknown ≠ advertised) |
| C3 | Memory edit/refresh wiring | `memory/flush`, `memory/rewrite` engine methods exist unwired | Settings surface for project memory; engine round-trip test |
| C4 | Dogfooding cadence | The best defect source we have; every fielded bug this cycle came from real use | Fixed weekly hours using WanCode on real work; defects → ledger → priority by frequency×severity |

## 4. Task split

| Owner | Items | Notes |
|---|---|---|
| **cc** (implementer default) | A3, B1 harness + run, B2 generator, C2, C3, CI lane split (§5) | Draft PRs per protocol |
| **codex** (reviewer default + QA owner) | Reviews of all of the above; A2 finalization (owns PR #29/#31); B1 scorecard adjudication; C1 design review before any code | Verdicts per protocol |
| **user** | A1 cert application (identity/payment — only you can); competitor accounts for B1; merge/release authorizations | A1 is the critical path for the next release |

Role swaps per-PR remain allowed and stated in the PR.

## 5. Process guardrails (keep, plus one change)

- PR review protocol, evidence tables, NOT-RUN discipline, mutation testing
  where assertions need teeth: unchanged.
- **CI lane split** (new): PR lane = wancode lib tests + frontend + clippy +
  migration-audit (target < 10 min feedback); full engine battery moves to a
  merge gate / nightly lane. A ~35-minute round-trip on every push is the
  single largest drag on iteration speed measured this cycle.

## 6. Sequence (indicative)

- **Week 1**: merge #29/#31/#35; user starts A1 application; CI lane split;
  A3 design.
- **Weeks 2–3**: B1 first full run + scorecard; A3 implementation; C2.
- **Weeks 3–6**: C1 design → implementation; B2 page; C3; v0.20 release —
  signed if the cert has arrived, honest-unsigned fallback otherwise.

## 7. Explicitly not doing (and why)

- Plugin marketplace — no ecosystem to serve yet; revisit on demand signal.
- Cloud execution/delegation — depends on infrastructure we don't control.
- Session sharing — gated on upstream login state we can never obtain.
