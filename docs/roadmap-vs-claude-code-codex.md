# WanCode 对标 Claude Code / Codex —— 差距分析与开发路线图

> 基线：WanCode v0.9.0（2026-07-18）
> 方法：读 WanCode 源码统计已暴露能力 + 扫 grok-build 引擎的扩展方法全集 + 对照 Claude Code / Codex 已知功能集。

## 0. 一个决定性的事实

| 指标 | 数值 |
|---|---|
| WanCode 暴露的 Tauri 命令 | 34 |
| grok-build 引擎的 `x.ai/*` 扩展方法总数 | **234** |
| WanCode 实际调用的扩展方法 | **7** |
| 引擎能力覆盖率 | **3.0%** |

**结论：WanCode 当前最大的机会不是"写新功能"，而是"把引擎里已经实现、但界面没接出来的能力暴露出去"。**

这一判断有先例支撑：本轮开发中「模型热切换」和「权限模式」都不是新写的——引擎早就有 `session/setModel` 和 `SetSessionMode`，只是界面没接。接线成本远低于从零实现。

已使用的 7 个：`exit_plan_mode`、`rewind/points`、`rewind/execute`、`session/info`、`session/rename`、`session/delete`、`session/search`。

---

## 1. 功能对标矩阵

图例：✅ 完备 ｜ 🟡 部分/简化 ｜ ❌ 缺失 ｜ ⚙️ 引擎已支持但界面未接

| 能力 | Claude Code | Codex | WanCode | 备注 |
|---|---|---|---|---|
| **核心对话** |
| 流式回复 / 思考过程 | ✅ | ✅ | ✅ | 思考块可折叠、新回合自动收起 |
| 工具调用可视化 | ✅ | ✅ | ✅ | v0.8.6 起扁平树状 |
| 图片输入 | ✅ | ✅ | ✅ | |
| @ 文件引用 | ✅ | ✅ | 🟡 | 简单文件列表，上限 4000；⚙️ 引擎有 `search/fuzzy/*` |
| 斜杠命令 | ✅ | ✅ | 🟡 | **7 条硬编码**；⚙️ 引擎有 `commands/list` |
| 自定义斜杠命令 | ✅ | 🟡 | ❌ | ⚙️ 同上 |
| ↑ 调取历史输入 | ✅ | ✅ | ❌ | ⚙️ `prompt_history` |
| **忙时排队输入** | ✅ | ✅ | ❌ | ⚙️ `queue/*`、`interject`——**日常最可感知的缺口** |
| 中断当前回合 | ✅ | ✅ | ✅ | 停止按钮 |
| **上下文管理** |
| 上下文用量显示 | ✅ | ✅ | ✅ | |
| **压缩 / compact** | ✅ | ✅ | ❌ | ⚙️ `compact_conversation`——**有仪表盘没有刹车** |
| 项目记忆（AGENTS/CLAUDE.md） | ✅ | ✅ | ✅ | 引擎原生注入 |
| 记忆编辑 / 刷新 | ✅ | 🟡 | ❌ | ⚙️ `memory/flush`、`memory/rewrite` |
| **权限与安全** |
| 权限模式菜单 | ✅ | ✅ | ✅ | v0.8.3 五档 |
| 逐次审批 | ✅ | ✅ | ✅ | 含内联 diff 审批 |
| 沙箱执行 | ✅ | ✅ | ❌ | Codex 有 seatbelt/landlock；WanCode 无 |
| **文件夹信任提示** | ✅ | ✅ | ❌ | ⚙️ `folder_trust/request`——WanCode 现在**默认自动打开用户主目录**，无信任确认 |
| 引擎主动提问 | ✅ | ✅ | ❌ | ⚙️ `ask_user_question`——**当前被兜底应答静默吞掉，用户看不到** |
| **Git** |
| 状态显示 | ✅ | ✅ | 🟡 | 自己 shell 调 git（曾因此闪黑框）；⚙️ `git/status` |
| 暂存 / 提交 / diff | ✅ | ✅ | ❌ | ⚙️ `git/stage`、`git/commit`、`git/diffs`、`git/discard` |
| 分支切换 | ✅ | 🟡 | ❌ | ⚙️ `git/branches`、`git/checkout` |
| **worktree 并行** | ✅ | ✅ | ❌ | ⚙️ `git/worktree/*`（10+ 方法） |
| 代码审查模式 | ✅ | ✅ | ❌ | ⚙️ `review`、`review/comment` |
| **任务与并发** |
| 后台任务 | ✅ | 🟡 | ❌ | ⚙️ `task/list`、`task/kill`、`task_backgrounded` |
| 子 Agent | ✅ | ❌ | ❌ | ⚙️ `subagent/list_running`、`subagent/get`、`subagent/cancel` |
| Todo / 计划面板 | ✅ | 🟡 | ✅ | |
| 计划模式 + 退出握手 | ✅ | 🟡 | ✅ | v0.7.0 |
| 定时任务 | ❌ | ❌ | ❌ | ⚙️ `scheduler/*`——**双方都没有，可作差异化** |
| **扩展生态** |
| MCP 接入 | ✅ | ✅ | 🟡 | 改 TOML + 重开会话；⚙️ `mcp/toggle`、`toggle_tool`、`server_status`、`auth_trigger` |
| MCP OAuth 授权 | ✅ | ✅ | ❌ | ⚙️ `mcp/auth_trigger` |
| Skills | ✅ | ❌ | 🟡 | 列表/新建/编辑；⚙️ `skills/toggle` 启停 |
| Hooks | ✅ | ❌ | ✅ | v0.7.1 |
| 插件 / 市场 | ✅ | ❌ | ❌ | ⚙️ `plugins/*`、`marketplace/*` |
| **会话** |
| 历史 / 恢复 / 搜索 | ✅ | ✅ | ✅ | |
| 检查点回滚 | ✅ | 🟡 | ✅ | 三模式，**强于 Codex** |
| 会话 fork | 🟡 | ❌ | ❌ | ⚙️ `session/fork` |
| **多工作区切换** | ✅ | ✅ | ❌ | ⚙️ `session_summaries/workspace_list`；WanCode 单工作区 |
| 会话分享 | ✅ | 🟡 | ❌ | ⚙️ `share_session` |
| **终端** |
| 命令输出查看 | ✅ | ✅ | 🟡 | 只聚合 stdout，只读 |
| 交互式 PTY | ✅ | ✅ | ❌ | ⚙️ `terminal/pty/*`（create/input/resize/load） |
| **模型** |
| 多模型 / 第三方端点 | ❌ | ❌ | ✅ | **WanCode 独有** |
| 界面内配 Key + 测试连接 | ❌ | ❌ | ✅ | **WanCode 独有** |
| 会话中热切换模型 | ✅ | ✅ | ✅ | v0.8.2 |
| 推理强度选择 | ✅ | ✅ | ❌ | Claude 有 High/Medium；WanCode 无 |
| **云端** |
| 云端执行 / 委派 | ✅ | ✅ | ❌ | ⚙️ `cloud/env/*`、`cloud/terminate` |

---

## 2. UI / 布局对标

### 已对齐（v0.8.5–v0.9.0 完成）
- 左栏层次：新建置顶 → 导航 → Recents → 底部工作区行
- 底部 composer：`+` 菜单、工作区、模型、权限模式、发送
- 工具调用扁平树状（`●` 状态点 + `⎿` 输出）
- 中性灰阶 + Inter 字体 + 克制标题层级
- 消息悬停操作行（复制）
- GFM 表格渲染

### 仍有差距
| 项 | Claude Code | WanCode | 影响 |
|---|---|---|---|
| **Home 仪表盘** | 会话数/消息数/token/活跃天数/连续天数/高峰时段/常用模型 + 热力图 | 只有标题 + 建议 | 中——⚙️ `session_summaries/*` 可支撑 |
| **多项目 Recents** | 跨项目列出 | 仅当前工作区 | 高——切项目要重新选文件夹 |
| 推理强度选择器 | `Opus 4.8 · High` | 无 | 中 |
| 状态栏（成本/用量） | 可自定义 statusline | 仅上下文 % | 低 |
| 键盘快捷键 | Esc 中断 / ↑ 历史 / Ctrl-B 后台 | 极少 | 中 |
| 消息操作 | 复制/固定/朗读 | 仅复制 | 低 |

---

## 3. 路线图（按 价值 ÷ 成本 排序）

### Tier 1 —— 高频可感知，引擎已就绪，改动小
> 这一层每项都是"接线"而非"造轮子"，预计每项 0.5–1 天。

1. **忙时排队输入**（`queue/*`、`interject`）
   当前 Agent 干活时输入框事实上不可用，想补充一句只能等。Claude Code 和 Codex 都支持边跑边排队。**日常最可感知的缺口。**
2. **上下文压缩 `/compact`**（`compact_conversation`）
   现在只显示"上下文 10%"却没有任何应对手段——**有仪表盘没有刹车**。长会话必然撞墙。
3. **命令列表接引擎**（`commands/list`）
   7 条硬编码 → 真实命令注册表，顺带获得自定义命令能力。
4. **↑ 调取历史输入**（`prompt_history`）
   成本极低，两家都有。
5. **修 `ask_user_question` 被吞**
   引擎主动提问目前被兜底应答静默丢弃，用户永远看不到。属**正确性问题**而非增强。

### Tier 2 —— 结构性补强
6. **真 Git 面板**（`git/status|stage|unstage|diffs|commit|discard|branches|checkout`）
   替掉自己 shell 调 git 的实现（正是闪黑框那类 bug 的来源），并补齐暂存/提交/分支切换。
7. **后台任务 + 子 Agent 面板**（`task/*`、`subagent/*`）
   引擎已在发 `task_backgrounded`/`task_completed` 事件，**WanCode 现在直接忽略**。
8. **MCP 实时管理**（`mcp/toggle`、`toggle_tool`、`server_status`、`auth_trigger`）
   摆脱"改 TOML + 重开会话"，并补上 OAuth 授权流程。
9. **多工作区切换**（`session_summaries/workspace_list`）
   Claude Code 的 Recents 跨项目；WanCode 换项目要重选文件夹。
10. **模糊文件搜索 / 内容搜索**（`search/fuzzy/*`、`search/content`）
    改善 @ 引用体验，并提供全局搜索。
11. **文件夹信任提示**（`folder_trust/request`）
    我在 v0.8.2 加了"启动自动打开主目录"，**等于跳过了信任确认**——安全上应补。

### Tier 3 —— 差异化投入
12. **Git worktree 并行 Agent**（`git/worktree/*`）——两家高级用法的核心，引擎支持完整。
13. **交互式 PTY 终端**（`terminal/pty/*`）——从只读输出升级为真终端。
14. **代码审查模式**（`review`、`review/comment`）——对标 `/review`。
15. **定时任务**（`scheduler/*`）——**Claude Code 与 Codex 均无**，可作真差异化。
16. **Home 仪表盘**（`session_summaries/*`）——对标 Claude Code 首页统计。
17. **会话 fork**（`session/fork`）、**分享**（`share_session`）。

### 暂不建议
- **云端执行**（`cloud/*`）：依赖 xAI 云基础设施，对第三方模型场景无意义。
- **插件市场**（`marketplace/*`）：生态尚未形成，投入产出比低。
- **沙箱**：Windows 侧缺乏 seatbelt/landlock 对应物，成本高。

---

## 4. WanCode 已领先的地方（应继续放大）

1. **多模型**——Claude Code 只有 Claude，Codex 只有 OpenAI。WanCode 支持智谱/DeepSeek/任意 OpenAI 兼容端点，**这是最大的结构性优势**。
2. **界面内配 Key + 测试连接**——两家都要改配置文件。
3. **密钥进系统钥匙串**——两家多为明文配置/环境变量。
4. **三模式时光机**（全部/仅对话/仅文件）——比 Codex 的回滚更细。
5. **MCP 可视化配置**、**中英双语**、**亮/暗主题**。

---

## 5. 建议的下一步（三件事）

若只做三件，按此顺序：

1. **忙时排队输入** —— 最高频的体感缺口，成本最低。
2. **`/compact` 上下文压缩** —— 长会话的硬约束，现在无解。
3. **真 Git 面板** —— 顺带清掉自己 shell 调 git 的技术债。

三件都属"接引擎既有能力"，合计预计 2–3 天，可发一个 v0.10.0。

---

## 附：本文事实来源

- WanCode 命令清单：`src-tauri/src/lib.rs` 的 `invoke_handler`
- 引擎扩展方法全集：`grep -rhoE '"x\.ai/[a-z_/]+"' grok-build/crates/codegen`
- ExtMethod 兜底行为：`src-tauri/src/agent.rs` 的 `AcpClientMessage::ExtMethod` 分支
- Claude Code / Codex 功能集：基于公开文档与实际使用，标注为 🟡 的项存在版本差异，落地前建议再确认。
