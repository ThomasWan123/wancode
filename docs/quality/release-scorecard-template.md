# WanCode Release Quality Scorecard — vX.Y.Z

> Candidate commit: `<sha>`  
> Engine commit: `<sha>`  
> Test-plan revision: `<sha/path>`  
> Window: `<start> — <end>`  
> Decision: GO / NO-GO / CONDITIONAL

## 1. Executive result

- P0 open:
- P1 open:
- Core golden tasks passed:
- Environment matrix completed:
- Upgrade pairs completed:
- Competitive tasks completed:
- Known waivers:

## 2. Automated gates

| Gate | Result | Evidence |
|---|---|---|
| Frontend typecheck/tests/build | NOT-RUN | |
| Rust Clippy/unit/canary | NOT-RUN | |
| Provider contracts 4a/4b | NOT-RUN | |
| Surface identity/isolation | NOT-RUN | |
| Effective-tree and lock audit | NOT-RUN | |
| Packaged GUI E2E | NOT-RUN | |
| Update E2E | NOT-RUN | |
| Chaos/security suite | NOT-RUN | |
| Performance regression | NOT-RUN | |
| Accessibility smoke | NOT-RUN | |

## 3. Environment matrix

| Environment | Result | Evidence |
|---|---|---|
| Windows 11 fresh install | NOT-RUN | |
| Windows 11 upgrade | NOT-RUN | |
| Windows 10 release candidate | NOT-RUN | |
| English locale | NOT-RUN | |
| Simplified Chinese + IME | NOT-RUN | |
| 100% / 150% / 200% scale | NOT-RUN | |
| GLM | NOT-RUN | |
| DeepSeek | NOT-RUN | |
| Generic OpenAI-compatible | NOT-RUN | |

## 4. P0 invariants

| Invariant | Result | Evidence |
|---|---|---|
| Chat has no local extensions for its complete lifetime | NOT-RUN | |
| Code retains the full expected toolchain | NOT-RUN | |
| Provider endpoint and credential identity are exact | NOT-RUN | |
| Git/worktree operations never target another repository | NOT-RUN | |
| Binding migration never silently reclassifies a session | NOT-RUN | |
| Approval and blocked-state parsing fail closed | NOT-RUN | |
| Upgrade preserves configuration, sessions, and credentials | NOT-RUN | |

## 5. Golden and competitive tasks

| Task | Result | Evidence / score |
|---|---|---|
| First run and setup | NOT-RUN | |
| Chat conversation and image | NOT-RUN | |
| Code edit and test | NOT-RUN | |
| Session restore and rewind | NOT-RUN | |
| Git/review/worktree delivery | NOT-RUN | |
| Long task, interject, cancel | NOT-RUN | |
| Competitive B01–B07 | NOT-RUN | |

## 6. Reliability and performance

| Metric | Baseline | Candidate | Budget | Result |
|---|---:|---:|---:|---|
| Cold start | | | +15% | NOT-RUN |
| Idle memory | | | | NOT-RUN |
| 100-turn memory | | | no unbounded growth | NOT-RUN |
| 100k-file open/search | | | | NOT-RUN |
| Automated flake rate | | | <1% | NOT-RUN |
| GUI E2E flake rate | | | <2% | NOT-RUN |

## 7. Defects and waivers

| ID | Severity | Summary | Status | Owner | Target | Waiver reason |
|---|---|---|---|---|---|---|

## 8. Artifact integrity

- Installer SHA-256:
- MSI SHA-256:
- Updater signature match:
- Direct download match:
- Mirror download match:
- `latest.json` version/URL/signature match:
- Rollback or interrupted-update result:

## 9. Sign-off

| Role | Name | Decision | Date | Notes |
|---|---|---|---|---|
| Product | | | | |
| Engineering | | | | |
| Quality | | | | |

Any `NOT-RUN` item remains unproven. It must not be rewritten as PASS without a durable evidence link.
