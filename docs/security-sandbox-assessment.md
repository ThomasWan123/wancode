# Windows 沙箱评估报告（v0.17-2）

> 状态：评估先行，本报告不伴随代码改动。修订：2026-07-22。
> 范围：WanCode 桌面端在 Windows 上执行 AI 发起的命令时的隔离能力。
> macOS/Linux 不在本期（Tauri 侧无用户，引擎上游自带 seatbelt/landlock 讨论）。

## 1. 现状暴露面（先说实话）

WanCode 今天对 AI 发起的执行**没有任何 OS 级隔离**，防线只有审批 UI：

| 通路 | 进程形态 | 现有防线 |
|---|---|---|
| 引擎 bash/命令工具（`xai-grok-tools` bash） | 引擎内 spawn 的子进程，继承 WanCode 完整用户权限 | 权限审批（manual/acceptEdits/auto/bypass 模式）；plan 模式只读 |
| 交互式 PTY 终端 | ptyctl 起的 shell，同样全权限 | 无（用户自己打字，视为用户操作） |
| MCP 服务器 | stdio 子进程（npx 等），随会话常驻 | 文件夹信任门（未信任仓库不加载仓库级 MCP） |
| Review/一键修等自动化 | 走同一引擎工具通路 | background_sessions 权限自动取消（后台会话写不进盘） |
| wancode 自身（git/gh CLI） | CREATE_NO_WINDOW 子进程 | 客户端代码写死的固定命令，无用户可注入的参数拼接 |

要点：**审批 UI 是决策层不是执行层**。用户点了"允许"（或开了 auto/bypass
模式）之后，命令以当前用户的全部权限跑——能读浏览器 cookie 目录、能改
启动项、能碰任意盘符。北极星指标里的"安全"目前完全建立在模型行为 +
用户判断上。

## 2. Windows 隔离选项对比

### 2.1 Job Object（作业对象）
- **能做**：杀进程树（防孤儿/逃逸子进程）、内存/CPU 上限、
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（WanCode 退出全家消失）、
  UI 限制（剪贴板/桌面切换）。
- **不能做**：文件系统/注册表/网络访问控制——Job 不是权限边界。
- **成本**：低。`CreateJobObject` + `AssignProcessToJobObject`，对
  引擎侧需要在 spawn 处注入（补丁点：xai-grok-tools bash 执行器，
  或更省事——把 **wancode 主进程自己**放进 Job 并设 kill-on-close，
  子孙自动继承）。
- **风险**：嵌套 Job 在 Win10+ 已支持，npx/node 链条兼容良好。

### 2.2 受限令牌（Restricted Token / Low IL）
- **能做**：`CreateRestrictedToken` 去特权 SID + 降完整性级别（Low IL）
  后 `CreateProcessAsUser`。Low IL 进程**写不了**用户大部分目录/注册表
  （读仍可），写入被 MIC 拦截。
- **不能做**：挡"读"（凭据窃取类还是能读到不少）；很多开发工具在
  Low IL 下直接工作不正常（npm 写 node_modules、git 写 .git 都会炸），
  **对编码 Agent 这是致命的**——工作区本身就要写。
- **变体**：写白名单 = 给工作区目录加 Low IL 可写 ACE
  （`SetNamedSecurityInfo` 加 mandatory label），实现"只能写工作区"。
  可行但 ACL 手术侵入用户目录，卸载残留风险高，需谨慎。

### 2.3 AppContainer
- **能做**：最强隔离（能力 SID 白名单，文件/注册表/网络全默认拒绝）。
- **不能做**：几乎跑不了真实开发工具链——cargo/npm/git 在 AppContainer
  里的兼容性是灾难级，微软自家 Windows Sandbox/WDAG 都不走这条路给
  开发者用。**排除**。

### 2.4 网络出口控制（WFP / 防火墙规则）
- **能做**：`netsh advfirewall` 或 WFP API 按进程路径拦出站——
  "AI 执行的命令不许联网（模型/MCP 除外）"是用户能理解的强承诺。
- **不能做**：按 Job/令牌粒度拦（防火墙按可执行文件路径匹配，
  cmd.exe 一刀切会误伤用户自己的终端）。
- **务实版**：只对**引擎 spawn 的进程树**做（配合 2.1 的 Job +
  WFP provider per-appid），实现成本中等，二期再说。

### 2.5 Windows Sandbox / Hyper-V 容器
- 整机级隔离，秒级冷启不现实（每命令 10s+），工作区共享要 mapped
  folder。适合"高危命令单发"场景，不适合 Agent 高频循环。**排除**。

## 3. 推荐方案（分两期）

### 一期（v0.17.x，落地成本 ≤2 天）：Job Object 全家桶
1. wancode 启动时把自身放入 Job，设 `KILL_ON_JOB_CLOSE` +
   内存上限（如 4GB/子进程）——**应用退出 = AI 起的所有进程树必死**，
   根治孤儿 ping/node 进程（smoke 期间已多次观察到残留）。
2. PTY/MCP 子进程自动继承，无需动引擎补丁。
3. 顺手：任务面板显示 Job 内活跃进程数，杀任务=TerminateJobObject
   子 Job（比现在按 pid 杀更可靠）。

**明确不承诺**：一期不提供文件/网络访问控制；审批 UI 仍是唯一的
写操作防线。README 安全小节如实写。

### 二期（评估后决定）：工作区写边界
- 方向 A：Low IL + 工作区目录 mandatory label 白名单（2.2 变体）。
  先做兼容性 PoC：cargo build / npm i / git commit 在该配置下全绿
  才推进；任何一个不绿就放弃 A。
- 方向 B：不做 OS 强制，改做**事后审计**——引擎已有文件快照/回滚
  （rewind、worktree snapshot），把"工作区外的写"做成检测+告警
  （ETW 文件事件按进程树过滤）。弱于 A 但零兼容性风险。
- 倾向：先 B 后 A。编码 Agent 的现实是工具链要写的地方比直觉多
  （cargo home、npm cache、%TEMP%、钥匙串……），白名单战争大概率
  打不赢；审计+一键回滚与产品已有能力协同更好。

## 4. 与竞品的诚实对比

Claude Code CLI 在 Windows 上同样**没有** OS 级沙箱（macOS 有 seatbelt
profile）；Codex CLI 桌面场景亦以审批为主。一期 Job Object 落地后，
WanCode 在"进程生命周期治理"上即不落后于同类；文件/网络边界整个
品类都还没有好答案，二期不必抢跑。

## 5. 结论

- 立即做：一期 Job Object（收益/成本比最高，且修真实痛点——孤儿进程）。
- 观察做：二期方向 B（ETW 审计）PoC。
- 不做：AppContainer、Windows Sandbox、全局防火墙规则。
