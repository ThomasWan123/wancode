# Git worktree target-boundary evidence

Status: engine change merged to the durable `wancode-integration` commit below;
WanCode re-audit and independent review remain pending.

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
| Windows managed-path lookup | PASS — the Unix-only permissions test is platform-gated and the focused Windows regression executed successfully |
| Permanent engine boundary workflow | PASS — exact engine candidate ran the Windows path-lookup regression and both registered-target containment tests |
| WanCode repository-boundary unit test | PASS — linked worktree accepted; separate temporary repository rejected; all tracked sentinel files remained present |
| Combined WanCode + durable grok-build compilation/test | PENDING — exact-head GitHub CI will rerun against the merged engine input |
| Effective-tree manifest verification | PASS — durable engine commit, overlay hashes, porcelain set, and registered effective-tree digest match |
| Intentional-delta migration audit | PASS — A1 through A6; seven engine differences within the cumulative whitelist or admitted as a new file; wiring and Cargo lock unchanged |
| `git diff --check` | PASS |

## Pinned engine input

| Input | Value |
|---|---|
| grok-build base | `9e9adb62d3251c5cdafbbfe4dcc17ea83b411910` |
| grok-build candidate | `dc46d96fa0460295cf96db86900530743d2197f5` |
| Cargo lock overlay | unchanged |
| Effective-tree SHA-256 | `2bbc139eebf5d205f8b32efe3885fffbaf1090dfe63f61536ed8b9d2f750a621` |
| Migration audit mode | `intentional-delta` |

## Explicitly not run

- No destructive or out-of-boundary apply/remove operation.
- No packaged GUI smoke test.
- No WanCode merge, release, tag, asset upload, updater change, or repository-setting change.
- WanCode exact-head CI and independent CC re-review remain pending.
