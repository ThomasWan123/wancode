# 黄金任务清单（20 条）

> 北极星指标的执行载体：每次发版跑一遍，记分卡存 `docs/golden-runs/<版本>.md`。
> 验证方式：**AUTO**（smoke.ps1 / cargo test，零人工）｜**SEMI**（UI 坐标自动化，
> 本仓库脚本可复现）｜**MANUAL**（人工或真实使用中验证）。
> 原则：记分卡只填**有证据**的结论（日志/截图/磁盘断言），没跑就写 NOT-RUN，
> 绝不拿"应该没问题"充数。

## A. 首启与配置（指标：零配置首启 100%，配置残留 0%）

| # | 任务 | 通过标准 | 方式 |
|---|---|---|---|
| G1 | 真零配置首启 | 挪走 config.toml 启动：向导弹出，60 秒不崩 | SEMI（发版检查单①） |
| G2 | 向导贴 Key 一键配置 | 连接测试失败路径 fail-closed 零落盘（AUTO）；真实 401/真 Key 通过为人工实证 | AUTO+MANUAL |
| G3 | 老配置升级启动 | 已有配置直接可用：Connected、会话/MCP 在 | SEMI（发版检查单②） |
| G4 | 损坏配置不被吞 | config.toml 语法坏 → 报 Invalid，不误开向导 | AUTO（startup_gate_tests） |
| G5 | 配置事务性 | 中途失败零残留（钥匙串回滚、原子替换） | AUTO（config_txn_tests） |

## B. 核心会话循环（指标：任务成功率 ≥90%）

| # | 任务 | 通过标准 | 方式 |
|---|---|---|---|
| G6 | 会话启动 + 基本回复 | 真模型回合完成，回复落盘 | AUTO（smoke S1+S2） |
| G7 | 忙时排队 | 长任务中两条排队消息全部按序完成 | AUTO（smoke S3） |
| G8 | 回合中插话 | interject 当轮生效 | AUTO（smoke S4） |
| G9 | 会话恢复 | 同 id 续接，历史不缩水 | AUTO（smoke S6） |
| G10 | 权限审批链路 | 写文件弹审批，批准才落盘；auto 模式自动批 | MANUAL |
| G11 | 崩溃恢复 | 强杀后重启出恢复横幅，一键回会话 | SEMI |

## C. Git 与交付（指标：串仓库 0）

| # | 任务 | 通过标准 | 方式 |
|---|---|---|---|
| G12 | git 状态/贮藏不串仓库 | 显式 gitRoot：贮藏打在 fixture 而非宿主仓库 | AUTO（smoke S5 硬守卫） |
| G13 | Diff 工作台 | 变更列表/展开着色/单文件 stage/discard | SEMI |
| G14 | AI 审查 E2E | 未提交改动 → findings 结构化渲染，主聊天零污染 | SEMI |
| G15 | 一键修闭环 | findings → AI 核实修复 → 人工验收 diff 正确 | MANUAL |
| G16 | PR 闭环 | 分支 → App 内建 PR → URL 可开；PR 状态行显示 | MANUAL |
| G17 | worktree 安全 | apply 冲突预检亮牌；删除前快照可恢复 | AUTO（快照单测）+ MANUAL（预检 UI） |

## D. 稳定与运维（指标：崩溃恢复率 100%）

| # | 任务 | 通过标准 | 方式 |
|---|---|---|---|
| G18 | 进程树治理 | 强杀主进程 → AI 起的子进程 ≤5 秒全灭 | SEMI（脚本化对比 pid 集合） |
| G19 | 自动更新链路 | latest.json 版本正确、安装包镜像首 KB=MZ、旧版可升 | SEMI（发版检查单④） |
| G20 | 单测+金丝雀全绿 | cargo test --lib + engine_canary（引擎假设未漂移） | AUTO |

## 维护约定

- smoke.ps1 覆盖 G6–G9、G12（每次重构必跑）；G4/G5/G17(半)/G20 随 `cargo test`。
- 发版检查单四项对应 G1/G3/G20/G19，发版即得。
- 纯 MANUAL 项（G2/G10/G15/G16）尽量在开发自用（吃狗粮）中自然覆盖，
  记分卡引用当期真实使用证据。
