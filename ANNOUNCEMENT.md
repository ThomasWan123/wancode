# WanCode 发布公告 / Announcement

## 🎉 WanCode —— 你自己的多模型桌面 AI 编码助手

> A Claude-Code-style desktop coding agent that runs **any** model — Zhipu GLM, DeepSeek, or any OpenAI-compatible endpoint.

**English below.**

---

### 中文

**WanCode** 是一个对标 Claude Code 的桌面 GUI 编码助手，最大的不同是：**不绑定单一模型**——智谱 GLM、DeepSeek，或任意 OpenAI 兼容端点（含本地 Ollama）都能用。

Agent 引擎复用了开源的 [grok-build](https://github.com/ThomasWan123/grok-build)（Apache 2.0），GUI 用 Tauri 2 + React 重写，体积仅约 25MB。

#### ✨ 核心能力

- **多模型**：在设置里可视化添加模型 + API Key，一键**测试连接**确认可用；密钥存进系统钥匙串，不落明文
- **完整 Agent**：读写文件、执行命令、多轮工具调用、思考流
- **每次修改都要你批准**：diff 内联审批，改动落盘前你说了算
- **计划模式**：只读探索、先出计划再执行（含审批握手）
- **时光机回滚**：对话 / 文件 / 两者，三种模式
- **文件树 + @文件引用 + slash 命令 + 终端面板**
- **Git 助手 · MCP 可视化配置 · Hooks · Skills 系统**
- **项目记忆**（AGENTS.md）· 会话搜索/恢复/重命名 · 图片输入（视觉模型）
- **中英双语 · 亮/暗主题 · 应用内自动更新**

#### 📦 下载

从 [Releases](https://github.com/ThomasWan123/wancode/releases/latest) 下载 `.msi` 或 `-setup.exe`，安装后在 ⚙ 设置里填入你的模型和 API Key 即可。

#### 🙏 致谢

核心 Agent 运行时基于 [grok-build](https://github.com/ThomasWan123/grok-build)（SpaceXAI，Apache 2.0）。

---

### English

**WanCode** is a Claude-Code-style desktop coding agent. The key difference: it's **not tied to one model** — use Zhipu GLM, DeepSeek, or any OpenAI-compatible endpoint (including local Ollama).

The agent runtime reuses the open-source [grok-build](https://github.com/ThomasWan123/grok-build) (Apache 2.0); the GUI is a fresh Tauri 2 + React build, ~25MB.

#### ✨ Highlights

- **Multi-model**: add models + API keys visually in Settings, with a one-click **Test connection**; keys live in the OS keyring, never in plain text
- **Full agent**: read/write files, run commands, multi-turn tool use, thinking stream
- **Every edit needs your approval**: inline diff review before anything hits disk
- **Plan mode**: read-only exploration, plan first then execute (with an approval handshake)
- **Time-travel rewind**: conversation / files / both
- **File tree · @-file mentions · slash commands · terminal panel**
- **Git helper · visual MCP config · hooks · skills system**
- **Project memory** (AGENTS.md) · session search/resume/rename · image input (vision models)
- **Bilingual (zh/en) · light/dark theme · in-app auto-update**

#### 📦 Download

Grab the `.msi` or `-setup.exe` from [Releases](https://github.com/ThomasWan123/wancode/releases/latest), install, then add your model + API key in ⚙ Settings.

#### 🙏 Credits

Agent runtime based on [grok-build](https://github.com/ThomasWan123/grok-build) by SpaceXAI (Apache 2.0).

---

*WanCode is Apache 2.0 licensed. Contributions welcome.*
