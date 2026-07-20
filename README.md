<div align="center">

# WanCode

**多模型桌面 AI 编码助手 · Multi-model desktop AI coding agent**

支持智谱 GLM-5 / DeepSeek / 任意 OpenAI 兼容端点
Zhipu GLM-5 · DeepSeek · any OpenAI-compatible endpoint

</div>

---

## 简介 / Overview

WanCode 是一个类 Claude Code 的桌面 GUI 编码助手：理解代码库、读写文件、执行命令、Git 与 MCP 扩展，**核心区别是可以自由接入国产与第三方大模型**。

WanCode is a Claude-Code-style desktop coding agent — it understands your codebase, reads/writes files, runs commands, and extends via MCP. Its distinguishing feature is **first-class support for third-party / Chinese LLMs**.

Agent 引擎复用了开源的 [grok-build](https://github.com/ThomasWan123/grok-build)（Apache 2.0），GUI 用 Tauri 2 + React 重写，模型层抽象为 OpenAI 兼容 Provider。

## 功能 / Features

- 🤖 **多模型**：智谱 GLM-5.2 / GLM-5-Turbo / GLM-4-Flash、DeepSeek V3 / R1，或任意 OpenAI 兼容端点（Ollama、One-API 等）
- 💬 **流式对话**：Markdown 渲染、思考过程折叠、工具调用卡片
- 🔐 **权限审批**：每次文件修改前弹窗询问（询问 / 本会话允许 / 拒绝）
- 📝 **Diff 展示**：文件改动以 diff 呈现，批准后才落盘
- ⏪ **时光机回滚**：三种模式（对话 + 文件 / 仅对话 / 仅文件），基于引擎快照
- 📊 **上下文用量**：实时 token 用量条
- 🗂️ **会话管理**：历史侧栏、一键恢复重放、重命名、删除
- 🔌 **MCP 可视化配置**：设置页增删 MCP 服务器（stdio / HTTP）
- 🧠 **项目记忆**：自动注入工作区根目录的 `AGENTS.md`（兼容 `CLAUDE.md`、`.grok/rules/*.md`）
- 🚀 **一键配置**：首启向导选卡贴 Key 即可用；连接测试通过才保存，绝无半配置
- 🔍 **默认联网搜索**：智谱系配置后自动启用 web-search / web-reader MCP（配置零明文）
- 🌐 **中英双语界面**

## 快速开始 / Quick Start

> ⚠️ 请使用 **v0.12.1 及以上**版本。更早的版本在全新安装（从未配置过模型）时无法启动。
> Use **v0.12.1+**. Earlier versions fail to launch on a fresh install.

四步开始干活（无需碰任何配置文件）：

1. 从 [Releases](https://github.com/ThomasWan123/wancode/releases) 下载 `-setup.exe`（或 `.msi`）安装并启动
2. 首次启动自动弹出向导：**选择你的服务商卡片**（GLM Coding Plan / 智谱开放平台 / DeepSeek，或自定义 OpenAI 兼容端点）
3. **粘贴 API Key** —— 自动测试连接，通过才保存；智谱系 Key 会同时自动启用联网搜索（web-search MCP）
4. **打开一个项目文件夹**，开始对话

Four steps, no config files: install → pick your provider card in the first-run wizard → paste an API key (connection-tested before saving; Zhipu keys also enable web-search MCP automatically) → open a project folder.

**常见错误 / Common pitfall**：智谱 **Coding Plan**（包月订阅）与**开放平台**（按量计费）是不同端点、Key 不通用。向导里分成两张卡片——按你实际购买的类型选。
Zhipu's monthly *Coding Plan* and pay-as-you-go *Open Platform* use different endpoints with non-interchangeable keys — pick the card that matches what you bought.

### 高级：手工配置 / Advanced: manual config

不想用向导也可以直接编辑 `%USERPROFILE%\.grok\config.toml`（示例接入 DeepSeek）：

```toml
[models]
default = "deepseek-chat"

[model.deepseek-chat]
model = "deepseek-chat"
base_url = "https://api.deepseek.com/v1"
env_key = "DEEPSEEK_API_KEY"      # API Key 从环境变量读取，不落明文
api_backend = "chat_completions"
context_window = 65536
```

然后设置对应的 `*_API_KEY` 环境变量。注意：删光所有模型后应用会回到首次运行向导。

## 从源码构建 / Build from source

需要：Rust (MSVC toolchain)、Node.js、[protoc](https://github.com/protocolbuffers/protobuf/releases)，以及本仓库相邻目录下的 `grok-build`。

```powershell
# Windows：用 lld-link（VS2022 LLVM 组件）绕过 MSVC PDB 上限，并扩大栈
$env:RUSTFLAGS="-C link-arg=/STACK:16777216"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="lld-link"
npm install
npm run tauri build      # 出 MSI + NSIS 安装包
# 开发调试：npm run tauri dev
```

## 技术栈 / Tech Stack

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2 |
| 前端 | React 18 + TypeScript + Vite |
| Agent 引擎 | [grok-build](https://github.com/ThomasWan123/grok-build) crates (Rust) |
| 模型接入 | OpenAI 兼容 Provider 抽象层 |
| 通信 | Agent Client Protocol (ACP) over in-process channel |

## 致谢 / Acknowledgements

核心 Agent 运行时基于 **[grok-build](https://github.com/ThomasWan123/grok-build)**，遵循 Apache License 2.0。详见 [NOTICE](NOTICE)。

## 许可 / License

[Apache License 2.0](LICENSE) © WanCode contributors
