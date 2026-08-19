# WanCode v0.20.0

WanCode v0.20 extends the Windows desktop agent with document work, clearer
model controls, auditable compatibility evidence, and a safer update path.

## Highlights

- **Work surface:** import DOCX documents into a read-only local staging area,
  extract structured blocks in a bounded worker process, and preserve exact
  UTF-16 source anchors for fail-closed citation resolution.
- **Reasoning effort:** capability-gated effort choices follow the selected
  model instead of advertising unsupported controls.
- **Project memory:** refresh and rewrite operations are wired into Settings
  with explicit backend round trips.
- **Provider evidence:** the public compatibility matrix is generated from
  committed CI summaries and cannot promote missing live evidence to a pass.
- **Multi-source updater:** origin and gh-proxy manifests are checked
  independently, only the highest available version is eligible, and each
  candidate must pass the updater's pinned minisign verification before launch.
- **Cowork safety evidence:** the real-engine/worktree escape probe is included,
  while the Cowork product surface stays gated under the conservative tier.

## Release quality and limits

Frontend, Rust, provider, surface, parser-containment, and release-manifest
contracts are part of the release gate. The B1 three-product competitive round
is explicitly `NOT-RUN` because equivalent authenticated competitor surfaces,
frozen tasks, and an independent blind adjudicator were not all available. No
"better than Codex/Claude Code" result is claimed.

This personal-use release follows the approved honest-unsigned policy:
Windows Authenticode is not claimed. Update integrity is provided by the pinned
minisign key, published hashes, and the updater's signature verification.

## Download

Download the Windows x64 installer from the
[WanCode v0.20.0 release page](https://github.com/ThomasWan123/wancode/releases/tag/v0.20.0).

WanCode and its grok-build-derived runtime are licensed under Apache 2.0.
