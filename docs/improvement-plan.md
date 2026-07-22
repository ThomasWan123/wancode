# WanCode 改进计划（v0.12.2 → v0.17.0）

> 基础：外部评审（Codex）的版本化路线图 + 本项目实际开发中验证过的工程事实。
> 修订：2026-07-20。执行状态标记：☑ 已完成 ｜ ▶ 进行中 ｜ ☐ 未开始。

## 0. 与 Codex 方案的关系

**采纳其全部框架**：版本化交付、北极星从"引擎方法覆盖率"换成任务成功率/崩溃率/
恢复率、先封口再重构再补高级能力的顺序。以下只写**差异与增补**，未提及处即按
Codex 原案执行。

### 差异一：CI 首版不做全量构建
grok-build 工作区在 GitHub Actions 冷编译约 1 小时且依赖 protoc/lld 工具链。
首版 CI 只跑：TS 类型检查、Vite 生产构建、cargo fmt/clippy、**wancode 单测**、
版本一致性、发布脚本无绝对路径。全量 tauri build 放 nightly / release tag 触发。

### 差异二：首启矩阵自动化落在 Rust 层
配置校验已抽为可注入路径的纯函数（`validate_startup_models_at`），矩阵按行落
单测（已落 5 行）。UI 级自动化只保留一条"真零配置 smoke"——本项目的 UI 自动化
靠 Win32 坐标点击，维护成本高、误报多（本周三次把自己的坐标错误当产品 bug），
不适合承载矩阵。

### 增补一：引擎金丝雀测试（Codex 案未覆盖，本项目最独特的风险）
本客户端依赖大量**引擎未成文行为**，每次引擎升级都可能静默破坏：
- fuzzy 广播键名 `searchId`、插话去重键 `interjectionId`
- `mcp/list` camelCase 与 `mcp/toggle` snake_case 并存
- git/* 显式 `gitRoot` 时的行为（#83 的根修依赖它）
- scheduler 通知重播的幂等契约、fork 的 N/N+1 截断差
- 零模型启动 panic（我们的门控在为它兜底）

建立 `src-tauri/tests/engine_canary.rs`：把这些依赖写成断言（能单测的单测，
不能的至少断言引擎源码中的常量/结构存在）。**引擎 bump 时金丝雀先叫**，而不是
用户先崩。归入 v0.13.0，与 submodule 固定化同批。

### 增补二：崩溃恢复闭环（指标"崩溃恢复率 100%"缺执行机制）
- Rust 设 panic hook：崩溃时写 `~/.grok/wancode-crash.json`（时间/会话 id/工作区）。
- 下次启动检测到标记 → 顶部横幅"上次异常退出，恢复该会话？"一键恢复。
- 归入 v0.12.2（小改动，指标的地基）。

### 增补三：发版检查单固定化（本周教训直接转制度）
`scripts/release.ps1` 末尾追加强制清单输出，人工确认后才提示 gh 命令：
1. 真零配置首启 smoke（本周发现 v0.12.0 对新用户闪退——历史所有版本都没测过这条）
2. 老配置升级启动 smoke
3. 镜像拉 latest.json + 安装包首 KB（MZ 头）
4. 单测全绿
（长期由 CI/nightly 接管 1、2。）

### 增补四：CSP 收紧提前到 v0.13.0
Codex 放在 v0.17。但 CSP 目前为 null 是纯配置项修复，与重构同批做掉，成本一天内。
完整沙箱评估仍留 v0.17。

### 调整：v0.13 重构的安全网前置
拆 App.tsx（3000+ 行）/agent.rs（4000+ 行）之前，先把"黄金任务"里能脚本化的
核心 6 条固化为 `scripts/smoke.ps1`（零配置首启/发消息/排队+插话/git 状态与
stash/fuzzy/会话恢复）。**没有行为级安全网的重构等于盲拆**。拆分期间每合一个
功能域跑一遍。

## 1. 版本计划（含状态）

### v0.12.2 稳定性封口（进行中）
- ☑ 启动门控下沉 Rust：`MODEL_REQUIRED` / `MODEL_CONFIG_INVALID` 结构化错误，
  悬空 default 自动修复并落盘；前端契约=重开向导；矩阵纯配置行 5 单测全绿
- ▶ 配置事务化：内存生成完整 TOML → 临时文件 → 原子替换 → 钥匙串，任一步失败
  回滚本次新写入的钥匙串项；MCP 播种并入事务；抽纯函数 + 单测
  （模拟第二模型写失败无半配置 / API 失败零变化 / 损坏保原件）
- ☐ 崩溃恢复闭环（增补二）
- ☐ README 重写：快速开始=下载→双击→选卡→贴 Key→选文件夹；TOML 移"高级配置"；
  Coding Plan 与开放平台 Key 不通用写进常见错误
- ☐ 完成定义：矩阵全过 + 真零配置 smoke + README 与实际流程一致

### v0.13.0 工程基础
- ☑ smoke.ps1 安全网（调整项，先于拆分）—— 6 场景全过
- ☑ 拆 App.tsx（3881→1825 行，10 个域步 A 透传；顺手逮住 showSettings 漏传与
  ext_call 双注入 vs serde alias 两个真 bug，都已修并回归 smoke 6/6）
- ☑ 拆 agent.rs（步 B，v0.18-1~4）：3845→1032 行，10 模块——
  config_core(302)/provider_ops(489)/crash_recovery(48)/git_ops(574)/
  skills_ops(247)/engine_ops(841)/review_ops(165)/autotest(223)；
  agent.rs 只剩状态/生命周期/通知泵/ext 地基。每刀 smoke 全过。
- ☐ ipc/commands.ts 集中、前端消 any——后续迭代。
- ☑ grok-build 固定 commit（vendor/grok-build.lock + local.patch + 冻结
  Cargo.lock，非 submodule——引擎必须是兄弟目录才能吃 workspace 依赖继承）；
  bootstrap.ps1（幂等，已实测全新克隆+补丁链路，Windows 需 core.longpaths）；
  smoke/release 脚本去绝对路径（仓库根动态计算）
- ☑ CI（差异一的降级版：frontend tsc+build / 三处版本一致性 / windows 上
  按 vendor lock 克隆引擎 → clippy --no-deps -D warnings → lib 单测+金丝雀）
  ——首绿于 run 29824922748。前两跑翻的都是"本机缓存掩盖的干净环境差异"
  （clippy -D 波及引擎 crate；generate_context! 编译期要 dist 存在），
  这正是降级 CI 的价值所在。
- ☑ 引擎金丝雀 src-tauri/tests/engine_canary.rs 七条（alias 双注入前提、
  gitRoot 通道、零模型 panic 假设、interjectionId/searchId 键名、
  引擎 HEAD 与 lock 一致），本机+CI 双绿。
- ☑ CSP 收紧（增补四）：null → self 白名单 + data:/blob: 图片 + inline
  样式 + Tauri IPC；实测样式/IPC/粘贴图片预览无回归。

### v0.14.0 工作台 UI ——【已发布 2026-07-21】
双栏工作台（Diff 一级视图=git_diffs includePatch + 单文件 stage/unstage/
discard；文件查看=fs/read_file 行号视图）、命令面板 Ctrl+K（12 动作）、
全局快捷键、Transcript 三档。多栏拖拽与 Diff 行级评论未做（对标案的
"批量反馈"挪 v0.15 Review 一起）。发版检查单 4/4：零配置向导✓ 老配置
升级✓ 单测+金丝雀 18 绿✓ 镜像 MZ 头✓。
按 Codex 案：双栏/三栏预设（不做任意拖拽）、Diff 一级视图（文件列表/行级评论/
批量反馈）、文件查看+轻量编辑、快捷键+命令面板、Transcript 三档显示。
本项目补充：保留现单栏为回退布局；Diff 视图直接复用引擎 git/diffs + #83 的
显式 gitRoot 通道。

### v0.15.0 Review 与 Git 交付闭环 ——【核心已落地 2026-07-21】
- ☑ Review：只读后台子会话 + 结构化 JSON findings（工作台第三标签）。
  background_sessions 三层屏蔽（通知/权限/交互型 ext 请求）。吃狗粮实测
  两轮抓到 6 个自身真问题，3 个当场修。
- ☑ PR：git push -u + gh pr create（CREATE_NO_WINDOW + spawn_blocking），
  Git 面板一键，E2E 实测 pull/1 创建成功。
- ☑ 审查一键修（v0.15-3）：findings→"先核实再改"prompt 发主会话；
  自举实测：内嵌 GLM-5.2 修对全部 4 条自审发现。
- ☑ 行级评论进 Diff 视图（v0.15-4）：@@ 行号跟踪 + inHunk 哨兵
  （二轮自审抓到 off-by-N 当场修）。
- ☐ PR 状态/CI 查询——后续迭代。
- 已知：Git 面板 worktree 列表偶报 "hub error: neither $GROK_HOME nor
  $HOME is set"（既有独立问题，待查）。

### v0.16.0 并行任务与 Worktree ——【安全网已落地 2026-07-21】
- ☑ apply 冲突预检：git2 双侧改动集求交（比原计划的 serialize_changes
  更直接），重叠先亮牌确认；apply 保持 merge 红线。
- ☑ 删除前快照：未提交改动导出 patch 到 ~/.grok/wancode-wt-snapshots/
  （含未跟踪文件内容 + 恢复说明头），remove 三态确认取代无确认 force。
  快照逻辑有磁盘级单测。
- ☑ 附带根治 worktree/list hub error（GROK_HOME 兜底，见 lib.rs 注释）。
- ☑ 任务中心 WORKTREES 区（v0.16-2）：目录/分支+续接会话，四类后台
  工作聚合齐（bg 任务/子 Agent/定时/worktree），顶栏计数含 worktree。

### v0.17.0 预览与安全
- ☑ Web 预览标签（v0.17-1）：工作台第四标签，iframe 嵌本地 dev server
  （仅 localhost/127.0.0.1），CSP frame-src 白名单放行。已知限制：iframe
  聚焦时全局快捷键不冒泡（Web 平台固有）。
- ☑ Windows 沙箱评估报告（docs/security-sandbox-assessment.md）：
  结论=一期 Job Object 全家桶（kill-on-close 根治孤儿进程，≤2 天），
  二期先 ETW 审计 PoC 后再议 Low IL 写白名单；AppContainer/Windows
  Sandbox/全局防火墙排除。竞品在 Windows 同样无 OS 沙箱，不必抢跑。
- ☑ 一期 Job Object 落地（v0.17-3）：kill-on-close + 8GB/进程上限，
  强杀主进程实测 MCP node 子进程 4 秒内全灭，smoke 6/6 回归。

## 2. 黄金任务与指标
20 条黄金任务已落地为 docs/golden-tasks.md（AUTO/SEMI/MANUAL 三档验证方式），
首张记分卡 docs/golden-runs/v0.17.0.md（17✅/2◐/1⏭/0✗）。每版发布跑一遍。
指标目标不变：零配置首启 100%、配置残留 0%、任务成功 ≥90%、崩溃恢复 100%、
串仓库 0、新用户首次成功对话 <3 分钟（v0.12.1 实测约 2 分钟）。

## 3. 暂不做（合并两份清单）
云端执行、企业权限、插件市场、社交分享、观赏型统计首页、完整 IDE/LSP、
纯覆盖率导向的 RPC 接入、大规模视觉重做、xAI 门控/遥测类方法（审计详见
roadmap 附 B）。
