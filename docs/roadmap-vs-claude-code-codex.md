# WanCode 对标 Claude Code / Codex —— 差距分析与开发路线图

> 基线：WanCode v0.11.0 + 全量方法分类审计（2026-07-19，第二阶段启动版）
> 方法：读 WanCode 源码统计已暴露能力 + **对引擎全部 251 个 `x.ai/*` 字符串逐一分类**（dispatch 表核对、方向判定、门控检查）+ 对照 Claude Code / Codex 已知功能集。

## 0. 引擎覆盖率：换用真分母

此前的覆盖率用的是字符串 grep 出的原始计数（234→251），**那个分母是虚的**：里面混着 50 个假字符串（测试夹具、dispatch 前缀、`_meta` 能力键）、15 个 leader/内部管线方法、7 个 xAI 门控方法。全量分类审计后的诚实口径：

### 251 个字符串的去向

| 类别 | 数量 | 说明 |
|---|---|---|
| **REQ** — 真实现的客户端请求 | 127 | 90 未接 + 37 已接 |
| **NOTIF-OUT** — 引擎→客户端通知 | 34 | 监听即覆盖，已监听 9 |
| **NOTIF-IN** — 客户端→引擎通知 | 14 | 11 未接 + 3 已接 |
| **反向请求** — 引擎→客户端 ExtMethod | 3 | ask_user_question / exit_plan_mode / folder_trust，已全接 |
| ⛔ GATED — xAI 登录态/云后端门控 | 7 | billing、cloud/env/*、cloud/terminate、share_session |
| ⛔ INTERNAL — leader/debug/内部重载 | 15 | 含 mcp/call、mcp/sdk*（是引擎→客户端的反向桥，当请求发= method_not_found） |
| ⛔ FAKE — 测试夹具/前缀/`_meta` 键 | 51 | `x.ai/foo`、`x.ai/test`、`bashOutputNoColor`、`x.ai/folderTrust` 等 |

### 覆盖率（真分母 = 127 + 34 + 14 + 3 = **178**）

| 指标 | 数值 |
|---|---|
| 当前已覆盖（调用/监听/应答） | **52** |
| **当前覆盖率** | **29.2%** |
| 60% 门槛 | 107 个（还需 +55） |
| 第二阶段目标（接完全部值得接的） | **约 127 个 ≈ 71%** |

覆盖不到 100% 是**有意的**：REQ 里有约 20 个属于 xAI 遥测/账号管理（feedback、btw、rollout/survey、auth/*、privacy）或已证实的假能力（review 三件套），接上去违反 §3 的检查清单。

上一版的判断「最大的机会是接线不是造轮子」在 Tier 1–3 得到验证：16 项交付全部是接线。这一版把该判断推到底：**第二阶段就是把剩余 126 个可接项里值得接的约 75 个全部接完**。

---

## 1. 功能对标矩阵

图例：✅ 完备 ｜ 🟡 部分/简化 ｜ ❌ 缺失 ｜ ⛔ 不做（附理由）

| 能力 | Claude Code | Codex | WanCode | 备注 |
|---|---|---|---|---|
| **核心对话** |
| 流式回复 / 思考过程 | ✅ | ✅ | ✅ | 思考块可折叠、新回合自动收起 |
| 工具调用可视化 | ✅ | ✅ | ✅ | 扁平树状 |
| 图片输入 | ✅ | ✅ | ✅ | |
| @ 文件引用 | ✅ | ✅ | 🟡 | 文件列表，上限 4000。模糊搜索**故意未做**，见 §3 |
| 斜杠命令 | ✅ | ✅ | ✅ | T1.3 接引擎注册表 |
| 自定义斜杠命令 | ✅ | 🟡 | ✅ | 同上，随注册表自动获得 |
| ↑ 调取历史输入 | ✅ | ✅ | ✅ | T1.4，跨会话 |
| 忙时排队输入 | ✅ | ✅ | ✅ | T1.1 |
| 中断当前回合 | ✅ | ✅ | ✅ | |
| **上下文管理** |
| 上下文用量显示 | ✅ | ✅ | ✅ | |
| 压缩 / compact | ✅ | ✅ | ✅ | T1.2。此前是假实现：给模型发一句「请压缩」，压缩不了还更占上下文 |
| 项目记忆（AGENTS/CLAUDE.md） | ✅ | ✅ | ✅ | 引擎原生注入 |
| 记忆编辑 / 刷新 | ✅ | 🟡 | ❌ | ⚙️ `memory/flush`、`memory/rewrite` |
| **权限与安全** |
| 权限模式菜单 | ✅ | ✅ | ✅ | 五档 |
| 逐次审批 | ✅ | ✅ | ✅ | 含内联 diff 审批 |
| 沙箱执行 | ✅ | ✅ | ❌ | Windows 无 seatbelt/landlock 对应物 |
| 文件夹信任提示 | ✅ | ✅ | ✅ | T2.6，**本轮安全性最实质的一项**，见 §4 |
| 引擎主动提问 | ✅ | ✅ | ✅ | T1.5，此前被兜底应答静默吞掉 |
| **Git** |
| 状态显示 | ✅ | ✅ | ✅ | T2.1 改用引擎 git，连带清掉自己 shell 调 git 的技术债 |
| 暂存 / 提交 / diff / 丢弃 | ✅ | ✅ | ✅ | T2.1 |
| 分支切换 | ✅ | 🟡 | ✅ | T2.1 |
| worktree 并行 Agent | ✅ | ✅ | ✅ | T3.6。三方合并 UI 未做，冲突时如实列出 |
| 代码审查模式 | ✅ | ✅ | ⛔ | 引擎侧不存在此能力，见 §3 |
| **任务与并发** |
| 后台任务 | ✅ | 🟡 | ✅ | T2.2 |
| 子 Agent | ✅ | ❌ | ✅ | T2.2 |
| Todo / 计划面板 | ✅ | 🟡 | ✅ | |
| 计划模式 + 退出握手 | ✅ | 🟡 | ✅ | |
| 定时任务 | ❌ | ❌ | ✅ | T3.2。**两家都没有，WanCode 独有** |
| **扩展生态** |
| MCP 接入 | ✅ | ✅ | ✅ | T2.3 实时启停，不再需要改 TOML + 重开会话 |
| MCP OAuth 授权 | ✅ | ✅ | ✅ | T2.3 |
| Skills | ✅ | ❌ | 🟡 | 列表/新建/编辑；⚙️ `skills/toggle` 启停未接 |
| Hooks | ✅ | ❌ | ✅ | |
| 插件 / 市场 | ✅ | ❌ | ⛔ | 生态未形成，投入产出比低 |
| **会话** |
| 历史 / 恢复 / 搜索 | ✅ | ✅ | ✅ | |
| 检查点回滚 | ✅ | 🟡 | ✅ | 三模式，**强于 Codex** |
| 会话 fork（rewind-and-edit） | 🟡 | ❌ | ✅ | T3.3 |
| 多工作区切换 | ✅ | ✅ | ✅ | T2.4 |
| 会话分享 | ✅ | 🟡 | ⛔ | 需 xAI 登录态，本产品永远够不到，见 §3 |
| 内容搜索 | ✅ | ✅ | ✅ | T2.5，引擎 ripgrep 语义 |
| **终端** |
| 命令输出查看 | ✅ | ✅ | ✅ | |
| 交互式 PTY | ✅ | ✅ | ✅ | T3.5，xterm.js |
| **模型** |
| 多模型 / 第三方端点 | ❌ | ❌ | ✅ | **WanCode 独有** |
| 界面内配 Key + 测试连接 | ❌ | ❌ | ✅ | **WanCode 独有** |
| 会话中热切换模型 | ✅ | ✅ | ✅ | |
| 推理强度选择 | ✅ | ✅ | ❌ | 仍缺 |
| **云端** |
| 云端执行 / 委派 | ✅ | ✅ | ⛔ | 依赖 xAI 云基础设施，对第三方模型场景无意义 |

---

## 2. UI / 布局对标

### 已对齐
- 左栏层次：新建置顶 → 导航 → Recents → 底部工作区行
- 底部 composer：`+` 菜单、工作区、模型、权限模式、发送
- 工具调用扁平树状（`●` 状态点 + `⎿` 输出）
- 中性灰阶 + Inter 字体 + 克制标题层级
- 消息悬停操作行（复制、从这里分叉）
- GFM 表格渲染
- 终端面板两页：命令输出（只读）/ 交互终端
- 首页「其他项目的近期会话」（**刻意排除当前工作区**，避免与左栏重复）

### 仍有差距
| 项 | Claude Code | WanCode | 影响 |
|---|---|---|---|
| Home 统计仪表盘 | 消息数/token/活跃天数/连续天数/高峰时段 + 热力图 | 只有跨项目会话列表 | 低——观赏性 > 实用性 |
| 推理强度选择器 | `Opus 4.8 · High` | 无 | 中 |
| 状态栏（成本/用量） | 可自定义 statusline | 仅上下文 % | 低 |
| 键盘快捷键 | Esc 中断 / Ctrl-B 后台 | ↑ 历史已有，其余极少 | 中 |
| 消息操作 | 复制/固定/朗读 | 复制/分叉 | 低 |

---

## 3. 本轮结论：三项刻意不做

这是本轮最重要的方法论修正。**上一版路线图把「引擎里有 `x.ai/xxx` 方法」直接当成「这个功能可以接」，事实证明不成立。** 三项按原计划应做、实际不该做：

### 代码审查模式（原 Tier 3-14）—— 引擎里根本没有这个能力

- `x.ai/review` **本身没有实现**：前缀路由进去，匹配不到任何分支，返回 `method_not_found`。
- 唯一能调的 `review/comment` 把用户写的评论**硬编码成 `null` 丢弃**，只上传引用的代码位置。
- `recorded: true` 是写死的，与上传成功无关；GCS 没配就整个跳过，照样返回成功。
- **没有读取侧**，没有 list/get。

它是给 xAI 收集标注数据的**只写遥测管道**，不是代码审查功能。接上去会得到一个「能点、有反馈、实际什么都不做」的按钮。

### 会话分享（原 Tier 3-17）—— 本产品永远够不到

`share_session` 有四道门，第一道是 `require_xai_auth_for_share`（xAI 登录态），第二道是服务端下发且**默认为 false** 的 `sharing_enabled`。WanCode 用 GLM/DeepSeek，没有也不会有 xAI 登录态 —— 这个按钮 100% 报错。

### 模糊文件搜索（原 Tier 2-10 的一半）—— 复杂度不匹配收益

`search/fuzzy/*` 是 `open → change → status 通知 → close` 的**有状态流式协议**（open 返回 searchId，change 只 ack，结果以通知异步到达），复杂度比同步的 `search/content` 高一个量级。当前 `@` 引用在 4000 文件以内够用。为勾掉一个条目把复杂协议草草接一遍，比诚实地留着更糟。

> **给下一轮的检查清单**：把某个 `x.ai/*` 方法写进路线图前，先确认三件事 ——
> ① 它在 dispatch 里**真有实现**（不是前缀路由进去后 `method_not_found`）；
> ② 它的返回值**反映真实结果**（不是硬编码 `true`）；
> ③ 它的前置条件**本产品能满足**（不依赖 xAI 登录态 / 云基础设施 / 编译期戳记）。

---

## 4. 本轮踩到的引擎坑（写给后来接线的人）

这些不是抱怨，是接下来任何一次接线都可能再踩的。

1. **同级方法命名不一致。** `mcp/list` 要 camelCase `sessionId`，`mcp/toggle` 系列要 snake_case `session_id`。确认引擎无 `deny_unknown_fields` 后，`ext_call` 现在**两种都注入**。
   同类：`worktree/list` 的 `include_all` 是 snake_case，且 `repo` **没有 `serde(default)`，必须显式传（哪怕 null）**——官方 TUI 自己发的是 `includeAll`，那个过滤参数在人家客户端里是静默失效的。

2. **两套响应信封。** 多数方法包 `{result, error}`，但 `session/fork`、`share_session`、`review/*` 是**裸响应**。失败时 `result` 为 null —— 解析时若退回信封本身，会把引擎错误渲染成「不是 git 仓库」这类无害状态。**必须先判 `error`**。

3. **fork 的两处截断约定不一致（引擎 off-by-one）。** `chat_history`（模型真实上下文）按「保留 N 轮」截断，而 `updates.jsonl`（客户端回放用）按 `count > N + 1` 截，多留一轮。不处理的话**屏幕上会显示一轮模型根本不记得的对话**。客户端已加回放上限抵消。

4. **`apply` 的 `overwrite` 模式从不报冲突。** 它无条件把 worktree 内容写进主目录，用户在主目录对同一文件的改动会被**静默销毁**。必须用 `merge`。

5. **能力声明的位置。** `x.ai/folderTrust` 要放 `client_capabilities.meta`，放请求 meta 上静默无效。

6. **`option_env!` 的解析时机。** `GROK_VERSION` 要在**引擎 crate 编译时**存在，本 crate 的 `build.rs` 够不着；且 cargo 只从 cwd 向上找 `.cargo/config.toml`，兄弟目录读不到。

7. **PTY 的 `process_started/ended` 在 Windows 永不触发**（引擎里 `session_has_foreground_process` 硬编码 false），所以做不了「终端里还有进程在跑，确定关闭？」。

8. **scheduler 只有 `delete`。** 没有 create/list —— 创建由模型调 `scheduler_create` 工具完成，客户端只能从通知流重建视图，且**恢复会话时同一 taskId 会合法地来两遍**（updates.jsonl 重播 + 引擎 `announce_existing_tasks` 补发），必须幂等 upsert。

---

## 5. WanCode 已领先的地方（应继续放大）

1. **多模型** —— Claude Code 只有 Claude，Codex 只有 OpenAI。支持智谱/DeepSeek/任意 OpenAI 兼容端点，**最大的结构性优势**。
2. **界面内配 Key + 测试连接** —— 两家都要改配置文件。
3. **密钥进系统钥匙串** —— 两家多为明文配置/环境变量。
4. **定时任务** —— **Claude Code 与 Codex 均无。**
5. **三模式时光机**（全部/仅对话/仅文件）—— 比 Codex 的回滚更细。
6. **MCP 可视化配置**、**中英双语**、**亮/暗主题**。

---

## 6. 第二阶段路线图：覆盖率 29.2% → 60%+（目标 ~71%）

v0.11.0 已发布。第二阶段按功能域分 10 批接线，每批独立可测、独立提交。排序原则：用户可感知的在前，纯覆盖性的在后。

### P2.1 会话流控补全（7 项）★ 体感最强
`interject`（**回合中途插话引导**——Claude Code 的 steering，目前 WanCode 只能排队或打断）、`queue/edit`、`queue/reorder`、`queue/interject`、`toggle_plan_mode`（通知路径的模式切换）、`permissions/reset`、`yolo_mode_changed`（bypass 模式同步给引擎）。
验证：回合进行中插话，引擎在当前回合内响应；队列可编辑重排。

### P2.2 Skills 全套引擎化（6 项）
`skills/list|add|remove|toggle|config|reset`。现在的 Skills 管理是自己读写 `~/.grok/skills/` 文件——换成引擎 API，顺带获得**启停**能力（路线图老 B3 项）。
验证：toggle 后引擎注入的系统提示确实少了那个技能。

### P2.3 MCP 配置引擎化（4 项）
`mcp/upsert|delete|read_resource`、`session/update_mcp_servers`。摆脱「自己改 TOML + 重开会话」，配置改动即时生效。
验证：upsert 后 `mcp/list` 立刻可见，无需重开会话。

### P2.4 终端补全（7 项）
`terminal/pty/load`（**断线重连整段回放**——切会话回来终端还在）、`terminal/list|output|create|background|release|wait_for_exit`。
验证：切走再切回，PTY 内容完整回放（引擎 256KiB 环形缓冲）。

### P2.5 Git 补全（8 项）
`git/stash`、`git/info`、`git/current_commit`、`git/files`、`git/checkout_commit`、`git/checkout_session_head`、`git/git_repo_root`、`git/serialize_changes`。
验证：一次性仓库实测 stash/恢复（沿用 wttest 模式，不碰真项目）。

### P2.6 模糊文件搜索（4 项 + 1 通知）
`search/fuzzy/open|change|close` + `search/fuzzy/status` 通知。此前因「复杂度不匹配」推迟；现在覆盖率目标使流式协议值得做，@ 引用直接受益（去掉 4000 文件上限）。
验证：输入变化时结果流式更新，close 后通知停止。

### P2.7 fs/* 引擎化（5 项）
`fs/read_file|write_file|list|exists|delete_file`。文件树/预览改走引擎（当前是自己的 Tauri 命令），统一 .gitignore 语义。
验证：与现有文件树行为一致后替换。

### P2.8 会话管理补全（9 项）
`session/list|close|load_history|repair`、`session/updates`、`session_summaries/session_list|workspace_list_recent`、`workspaces/list`、`sessions/list`。
验证：close 后引擎侧会话资源释放；load_history 与现有侧栏数据一致。

### P2.9 记忆 + 杂项状态（7 项）
`memory/flush|rewrite`（老 B4 项）、`subagent/get`（子 Agent 详情钻取）、`recap`、`suggest`、`suggestPrompt`、`hooks/list`（hooks 面板改读引擎注册表）。
验证：rewrite 后 GROK.md 内容变化；hooks/list 与配置文件一致。

### P2.10 通知监听补全（约 15 项）+ worktree 深化（约 8 项）
监听：`models/update`、`config_changed`、`git_head_changed`、`mcp/init_progress`、`mcp/server_status`、`mcp_initialized`、`sessions/changed`、`session/prompt_complete`、`session/interjection`、`monitor_event`、`announcements/update`、`follow_ups`、`settings/update`、`search/content/status`、`hooks/event`（反向 RPC）。
worktree：`create`、`show`、`gc`、`db/stats`、`create_from_worktree(_sync)`、`worktree/status` 通知、`rehydrate`、`resolve_local_for_worktree_resume`。
验证：抽样触发（改 config 文件看 config_changed 到达；外部 git commit 看 git_head_changed）。

### 明确不接（约 20 个 REQ，理由入档）
- `review`、`review/comment`、`review/comment/delete` —— 假能力（§3）
- `feedback`、`btw`、`feedback/dismiss`、`rollout/survey`、`telemetry/*`×4 —— xAI 遥测汇
- `auth/*`×6、`getApiKey`、`setApiKey`、`privacy/setCodingDataRetention` —— xAI 账号管理，本产品密钥走系统钥匙串
- `bundle/*`×3、`pr/status`、`code/status` —— 依赖 xAI 侧基础设施/索引服务，价值存疑，留待验证
- GATED 7 个、INTERNAL 15 个 —— 定义即排除

### 完成后的账
52 已覆盖 + P2.1~P2.10 约 75 项 ≈ **127/178 ≈ 71%**，其中 60% 门槛（107）预计在 P2.8 完成时越过。

### 第二阶段之外（保持不变）
- 推理强度选择、键盘快捷键 —— 打磨项，与接线并行安排
- Home 统计仪表盘、worktree 三方合并 UI —— 先验证需求
- 沙箱执行 —— Windows 侧独立项目量级

---

## 附 A：本文事实来源

- WanCode 命令清单：`src-tauri/src/lib.rs` 的 `invoke_handler`（70 条）
- 分子统计：agent.rs 的方法字符串（43，剔除 `x.ai/folderTrust` 能力键）+ App.tsx/PtyTerm.tsx 监听的通知（9）= 52
- 引擎字符串全集：`grep -rhoE '"x\.ai/[A-Za-z0-9_/]+"' grok-build/crates/codegen | sort -u`（251 个）
- §3、§4 的每一条均来自阅读引擎源码 + 实测验证，非推测。具体验证方式见对应 commit message。
- Claude Code / Codex 功能集：基于公开文档与实际使用，标注 🟡 的项存在版本差异，落地前建议再确认。

## 附 B：251 个字符串的分类审计（2026-07-19）

逐一核对 dispatch 表（`acp_agent.rs` 的 ext_method/ext_notification 分支 + `extensions/` 各 handler）后的分类。**这个分母可复核——每个类别的判定依据都在引擎源码里，不是主观取舍。**

- **FAKE（51）**：以 `/` 结尾的 dispatch 前缀 19 个（`x.ai/git/`、`x.ai/mcp/` 等）；`_meta` 能力键/元数据键 25 个（`bashOutputNoColor`、`hunkTracker`、`sessionConfig`、`x.ai/folderTrust`、`x.ai/tool` 等——它们随请求元数据传递，从不作为方法分发）；纯测试夹具 7 个（`x.ai/foo`、`x.ai/test`、`x.ai/queue/bogus`、`x.ai/child_thing` 等）。
- **INTERNAL（15）**：`internal/*` 重载管线 7、leader 多客户端管线 3、`debug/*` 2、`mcp/call`/`mcp/sdk`/`mcp/sdk_call` 3——后三个看着像可调方法，实为**引擎→客户端**的 MCP 反向桥，当请求发直接 `method_not_found`。
- **GATED（7）**：`billing`、`cloud/env/*`×4、`cloud/terminate`（全部 `require_xai_auth` + SandboxClient）、`share_session`（xAI auth + 服务端 `sharing_enabled` 默认 false）。
- **REQ（127 = 90 未接 + 37 已接）**、**NOTIF-OUT（34）**、**NOTIF-IN（14）**、**反向 ExtMethod（3）**——构成真分母 178。
- 特别标注：`hooks/event`、`hooks/run` 是引擎→客户端的反向 RPC（客户端要实现并应答），归入 NOTIF-OUT 侧计数。
