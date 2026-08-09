# WanCode v0.19.0

WanCode is a multi-model desktop AI coding agent for Windows. It combines a native Tauri interface with a Rust agent runtime and supports Zhipu GLM, DeepSeek, and custom OpenAI-compatible endpoints.

## What is new in v0.19

Version 0.19 introduces two explicit session surfaces:

- **Chat** is designed for general questions and research. It runs from a private application directory, exposes only the built-in web tools, and disables local plugins, disk hooks, MCP, LSP, plugin skills, plugin commands, and extension-enabled subagents for the complete session lifetime.
- **Code** is the full development environment. It retains repository access, terminal commands, Git workflows, hooks, skills, MCP, LSP, plugins, and subagents.

The selected surface is not a frontend preference. WanCode binds it to the session identity, persists it in a fail-closed sidecar, derives the current policy in the Rust backend, and requires an explicit engine handshake before exposing the session to the UI.

## Core capabilities

- Configure multiple model providers without editing source code.
- Read and edit project files with inline diff review and approval controls.
- Run terminal commands and multi-step tool workflows.
- Search, restore, rename, branch, and rewind sessions.
- Use Git helpers, MCP servers, hooks, skills, project instructions, and model-supported image input in Code sessions.
- Store provider credentials in the operating-system keyring.
- Receive signed in-app updates on Windows.

## Release quality

The v0.19 release passed the full frontend and Rust CI suite, engine routing and identity-chain tests, Chat extension fan-out regression tests, reproducible effective-tree audits, signed installer validation, and both fresh-install and existing-configuration GUI smoke tests.

Work and Cowork surfaces remain planned. They were intentionally deferred so the Chat and Code lifecycle boundary could ship as a complete, testable contract.

## Download

Download the signed Windows x64 installer or MSI package from the [WanCode v0.19.0 release page](https://github.com/ThomasWan123/wancode/releases/tag/v0.19.0).

## Credits

WanCode uses the open-source [grok-build](https://github.com/ThomasWan123/grok-build) agent runtime under the Apache License 2.0. The WanCode desktop application is also licensed under Apache 2.0.
