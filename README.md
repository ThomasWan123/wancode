<div align="center">

# WanCode

**A multi-model desktop AI coding agent with separate Chat and Code surfaces.**

Use Zhipu GLM, DeepSeek, or any OpenAI-compatible endpoint from one native desktop app.

[![Latest release](https://img.shields.io/github/v/release/ThomasWan123/wancode)](https://github.com/ThomasWan123/wancode/releases/latest)
[![CI](https://github.com/ThomasWan123/wancode/actions/workflows/ci.yml/badge.svg)](https://github.com/ThomasWan123/wancode/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/ThomasWan123/wancode)](LICENSE)

</div>

---

## Overview

WanCode is a native desktop coding agent inspired by Claude Code. It can understand a repository, read and edit files, run commands, review diffs, work with Git, and connect to external tools through MCP.

Unlike single-provider clients, WanCode treats model choice as a first-class feature. You can use Zhipu GLM, DeepSeek, local gateways, or other OpenAI-compatible services without changing the application architecture.

The desktop client is built with Tauri 2, React, and TypeScript. Its Rust agent runtime is based on the open-source [grok-build](https://github.com/ThomasWan123/grok-build) project and is pinned through a reproducible, audited vendor manifest.

## Current release: v0.20.1

[WanCode v0.20.1](https://github.com/ThomasWan123/wancode/releases/tag/v0.20.1) adds a usable **Work** surface, capability-aware model controls, auditable provider evidence, and verified multi-source updates while preserving the Chat/Code boundary introduced in v0.19.

| Surface | Intended use | Local capabilities |
|---|---|---|
| **Chat** | General questions, research, and web-assisted conversations | Uses a private application runtime directory and built-in web tools. Local plugins, disk hooks, MCP servers, LSP servers, plugin skills, plugin commands, and extension-enabled subagents are disabled for the complete session lifetime. |
| **Code** | Repository work and software development | Keeps the full coding toolchain, workspace access, Git integration, terminal access, hooks, skills, MCP, LSP, plugins, and subagents. |
| **Work** | Local document understanding | Imports DOCX files into a read-only staging area, extracts anchored blocks in a crash-contained worker, and keeps local extension capabilities disabled by default. |

Surface identity is bound when a session is created and stored in a fail-closed WanCode sidecar. Restored sessions must resolve to their original surface, and the engine must explicitly confirm that the requested policy was applied before WanCode exposes the session handle.

Cowork remains gated. v0.20 ships its real-engine escape probe and records the conservative isolation verdict, but does not expose an unsafe task-delegation surface.

## Highlights

- **Multi-model support** — Zhipu GLM, DeepSeek, and custom OpenAI-compatible endpoints, with a published [provider compatibility matrix](docs/provider-compatibility.md) generated from CI evidence.
- **Capability-aware controls** — reasoning effort appears only for models that declare support; project-memory refresh and edit controls are wired to the engine.
- **Document Work surface** — staged DOCX import, durable document recovery, verified read-only source identity, fail-closed UTF-16 anchors, bounded worker-process parsing, and citation-ready document context supplied to every Work turn.
- **Streaming conversations** — Markdown rendering, collapsible reasoning, and tool-call cards.
- **Approval controls** — ask, allow for the current session, or reject sensitive actions.
- **Inline diff review** — inspect proposed file changes before they are written.
- **Checkpoint rewind** — restore conversation state, files, or both.
- **Session management** — search, resume, rename, delete, and replay previous sessions.
- **Project context** — loads `AGENTS.md` and supports compatible project instruction files.
- **Developer tools** — file browser, terminal, Git helpers, MCP configuration, hooks, skills, and image input where supported by the selected model.
- **Safe first-run setup** — provider credentials are saved only after a successful connection test.
- **Automatic updates** — origin and transport-only mirror manifests are checked independently; the highest version wins and every installer remains bound to the pinned minisign trust key.

## Quick start

Prebuilt releases currently target Windows x64.

1. Download the NSIS `-setup.exe` or MSI package from [GitHub Releases](https://github.com/ThomasWan123/wancode/releases/latest).
2. Launch WanCode and choose a provider in the first-run wizard.
3. Paste your API key. WanCode tests the connection before saving the configuration.
4. Choose **Chat** for a restricted conversation session, **Code** for the complete development toolchain, or **Work** for local document understanding.

> **Zhipu endpoint note:** the monthly GLM Coding Plan and the pay-as-you-go Open Platform use different endpoints and non-interchangeable API keys. Select the provider card that matches your subscription.

### Manual model configuration

The first-run wizard is recommended. Advanced users can edit `%USERPROFILE%\.grok\config.toml` directly:

```toml
[models]
default = "deepseek-chat"

[model.deepseek-chat]
model = "deepseek-chat"
base_url = "https://api.deepseek.com/v1"
env_key = "DEEPSEEK_API_KEY"
api_backend = "chat_completions"
context_window = 65536
```

Set the referenced environment variable before launching WanCode. Removing every configured model returns the application to the first-run wizard.

## Security model

WanCode applies the active surface policy in the Rust backend; the frontend only displays the selected surface. Chat isolation is enforced per session inside the engine and covers initial setup, restore, reload, broadcast, hooks, MCP, LSP, plugin commands, and subagent creation.

The policy is designed to fail closed on missing, corrupt, unsupported, or conflicting session bindings. It does not claim to protect against an attacker who already controls the local operating-system account.

See [Security sandbox assessment](docs/security-sandbox-assessment.md) and the [v0.19 design](docs/design/v0.19-layered-surfaces.md) for the detailed threat model and acceptance criteria.

## Build from source

### Requirements

- Rust with the MSVC toolchain
- Node.js
- [Protocol Buffers compiler (`protoc`)](https://github.com/protocolbuffers/protobuf/releases)
- Visual Studio 2022 LLVM tools for `lld-link`

The bootstrap script clones the engine into a sibling directory, checks out the exact commit registered in `vendor/grok-build.lock`, applies the audited wiring input, and validates the resulting effective tree.

```powershell
powershell -File scripts/bootstrap.ps1

$env:RUSTFLAGS = "-C link-arg=/STACK:16777216"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "lld-link"

npm run tauri build
```

Useful development commands:

```powershell
npm run tauri dev
npm test
cargo test --manifest-path src-tauri\Cargo.toml -j 1 --lib
powershell -File scripts/smoke.ps1
```

## Architecture

| Layer | Technology |
|---|---|
| Desktop shell | Tauri 2 |
| Frontend | React 19, TypeScript, Vite |
| Agent runtime | [grok-build](https://github.com/ThomasWan123/grok-build) Rust crates |
| Model integration | OpenAI-compatible provider abstraction |
| Agent transport | Agent Client Protocol (ACP) over an in-process channel |

## Project status

- **Latest stable release:** [v0.20.1](https://github.com/ThomasWan123/wancode/releases/tag/v0.20.1)
- **Available surfaces:** Chat, Code, and Work
- **Gated surface:** Cowork
- **Platform:** Windows x64
- **License:** Apache License 2.0

## Acknowledgements

WanCode's core agent runtime is based on [grok-build](https://github.com/ThomasWan123/grok-build), distributed under the Apache License 2.0. See [NOTICE](NOTICE) for attribution details.

## License

[Apache License 2.0](LICENSE) © WanCode contributors
