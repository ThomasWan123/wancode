# Git worktree target-boundary evidence

Status: implementation complete in isolated worktrees; independent review and
merge remain pending.

Scope: bind worktree apply/remove operations to a registered managed worktree
owned by the current session repository. This document intentionally records
the security property and verification results without publishing an
operational misuse recipe.

## Defense layers

| Layer | Contract | Result |
|---|---|---|
| WanCode command boundary | Apply/remove requests carry the current session repository identity | PASS |
| WanCode capability boundary | A caller-provided repository identity must resolve to the same Git common directory as the session | PASS |
| WanCode read/preflight boundary | Listing, precheck, and snapshot do not expose or accept a worktree from another repository | PASS |
| grok-build target resolution | Write operations accept only records present in the managed-worktree database | PASS |
| grok-build repository binding | When an expected source is supplied, the managed record must belong to that repository | PASS |
| Compatibility | Existing serialized requests may omit the new optional identity field; managed-record enforcement still applies | PASS |

## Non-destructive verification

All boundary tests used disposable temporary repositories. No apply, remove,
recursive-delete, or other destructive boundary probe was executed.

| Check | Result |
|---|---|
| grok-build registered-target unit tests | PASS — unmanaged temporary directory rejected; sentinel remained unchanged; registered same-source ID/path accepted; different source rejected |
| Repository-binding mutation check | PASS — bypassing the source comparison made the focused test fail; restoring the guard returned it to green |
| Windows managed-path lookup | PASS — code check covers the path lookup change; full crate tests are blocked by an unrelated pre-existing Unix-permissions test compilation issue on Windows |
| WanCode repository-boundary unit test | PASS — linked worktree accepted; separate temporary repository rejected; all tracked sentinel files remained present |
| Combined WanCode + modified grok-build compilation/test | PASS — exact WanCode boundary test passed against the modified engine tree |
| Effective-tree manifest verification | PASS — engine commit, overlay hashes, porcelain set, and registered effective-tree digest match |
| Intentional-delta migration audit | PASS — A1 through A6; five engine files within the cumulative whitelist; wiring and Cargo lock unchanged |
| `git diff --check` | PASS |

## Pinned engine input

| Input | Value |
|---|---|
| grok-build base | `9e9adb62d3251c5cdafbbfe4dcc17ea83b411910` |
| grok-build candidate | `1b491e64e94d22e53be50c09fb980bdc79af2953` |
| Cargo lock overlay | unchanged |
| Effective-tree SHA-256 | `102c908fb123a9096ea8abef335bcbce0327bdb1c4a80984e34e159288998e53` |
| Migration audit mode | `intentional-delta` |

## Explicitly not run

- No destructive or out-of-boundary apply/remove operation.
- No packaged GUI smoke test.
- No merge, release, tag, asset upload, updater change, or repository-setting change.
- GitHub CI and independent CC review are pending until both Draft PRs exist.
