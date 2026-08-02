# #126 设计稿：vendor 补丁迁移自有 fork（v0.19 基础设施）

> 状态：**设计评审中，未开工迁移**。评审通过前不做任何迁移动作。
> 现状事实：引擎 = fork `ThomasWan123/grok-build` @ `b189869b7755d2b482969acf6c92da3ecfeffd36`（上游 xai-org 开源基线）；wancode 侧本地补丁 `vendor/grok-build-local.patch` **7153 行 / 30 个文件**，由 `bootstrap.ps1` 在 clone 后套用。#126 的目标：把补丁内容变成 fork 上的**真实提交**，本地补丁降回零（或仅保留紧急逃生通道）。

## 0. 目标与不变量

- 目标：`vendor/grok-build-local.patch` → 空（或 <50 行纯构建接线）；全部引擎改动以 fork 提交形式存在、可 blame、可回滚、可逐条评审。
- 不变量（迁移全程成立）：
  - **纯基建，零行为变化**——迁移前后引擎逐字节等价（见 §3 每批验收）；
  - wancode 构建始终由 `vendor/grok-build.lock` 的 `repo+commit` 唯一决定；
  - 迁移期间产品发布不被阻塞（lock 随批次前移，任一批失败可退回上一 commit）。

## 1. fork 与上游同步策略

- fork 分支模型：
  - `upstream-mirror`：只读镜像上游 default 分支，仅 fast-forward，**永不**含我方提交；
  - `wancode-integration`：集成分支 = 上游基线 + 我方提交，wancode lock 只允许指向此分支上的 commit；
  - 主题分支 `wc/<topic>` 合入 integration（merge，不 rebase 已发布历史）。
- 同步节奏：**按需**（上游有需要的修复/特性时），不做定时同步——每次同步是一次显式工程事件：
  1. `upstream-mirror` fast-forward 到上游目标 commit；
  2. 新建 `sync/<date>` 分支：merge upstream-mirror 入 integration 副本，解冲突；
  3. 产出**同步审计报告**（见 §5）后走 PR 评审合回 integration；
  4. wancode 侧 lock bump PR + 全量 CI + smoke，绿了才算同步完成。
- 冲突策略：我方提交按 §3 的域分组，冲突按域指派回原域负责批次的测试重跑。

## 2. commit 固定与回滚

- 固定：沿用 `vendor/grok-build.lock`（repo + 40 位 commit），**不引入浮动分支引用**；每个 wancode release 在 fork 打标签 `wancode-engine/vX.Y.Z` 指向当时 lock commit（发布脚本自动化）。
- 不可变性：`wancode-integration` 与 `upstream-mirror` 开启分支保护，**禁止 force-push**；错误提交用 revert 前进式修复。
- 回滚：
  - 应用层回滚 = wancode 仓库 revert lock bump 提交（一步，CI 自动验证旧 commit 可构建）；
  - 引擎层回滚 = integration 上 revert + 新 lock bump；
  - 迁移期回滚 = 每批独立（§3），退回上一批的 lock commit 即完全恢复，无半迁移状态。

## 3. 7153 行 / 30 文件补丁的分批迁移

> 纪律：每批 = fork 主题分支 → fork PR（可逐 hunk 评审）→ 合入 integration → wancode 缩减 patch + lock bump 同一 PR → 全量 CI + smoke 6/6 → 下一批。任何一批红即停。

| 批 | 域 | 内容（补丁内对应段） | 独立验收 |
|---|---|---|---|
| B1 | 构建接线 | workspace members 加 wancode、xai-proto-build 的 protoc/Windows 修复 | wancode 全量构建 + CI 三 job |
| B2 | 模型身份 | catalog_model_id 持久化、resolve_override_ungated、modelBlock/ACP meta、双端点路由 | Gate 1 + model_identity_e2e + model_block_over_acp |
| B3 | 图片转述管线 | transcribe_images_enabled、describe 管线、尺寸门、降级垫底、HISTORY_IMAGE_OMITTED | 4b 套件（转述/内联）+ 图片专项冒烟 |
| B4 | 兼容性补丁杂项 | 429 文案、object 字段宽容化、4v-flash max_tokens 等历史兼容修复 | 4a 套件（错误组）+ 对应单测 |
| B5 | 引擎测试资产 | model_endpoint_routing.rs、model_identity_e2e.rs 等补丁引入的测试文件 | CI 对应步直跑（此批合入后矩阵中 Gate 1 落点从"patch 引入"改为 fork blob 直链） |
| 收尾 | 清空 patch | bootstrap 跳过空 patch；compatibility.md 落点链接全部改 fork blob | 逐字节等价审计（见下）|

- **逐字节等价审计**（每批必做）：`clone fork@新commit`（不打 patch）与 `clone fork@旧commit + 旧patch裁剪后` 两棵树 `git diff --no-index` 必须为空（仅该批范围）；收尾批做全树对比。
- 批间 lock 前移即发布可用——迁移与产品发布互不阻塞。

## 4. CI 如何证明使用了指定 fork commit

现状：CI 按 lock clone 并 checkout `$commit`——但**没有断言**实际 HEAD。补三道（随 B1 落地）：

1. clone 步后新增断言：`git -C $engine rev-parse HEAD` 必须等于 lock 的 commit，否则 fail；
2. 断言 patch 套用状态与预期一致：迁移期 = 当前批次预期的裁剪版 patch 的 sha256；收尾后 = patch 文件为空或不存在，且 `git -C $engine status --porcelain` 只含预期改动（Cargo.lock 覆盖）；
3. 引擎 commit 写入合规摘要（COMPLIANCE_SUMMARY 增 `engine_commit` 字段）与发布证据（docs/evidence），与 compatibility.md 的落点链接闭环。

## 5. 紧急补丁通道与供应链审计

- 紧急通道（P0 引擎故障，fork 流程来不及走）：
  - `vendor/grok-build-local.patch` 机制**保留为逃生舱**，常态为空；启用需在 patch 头部写明事故编号与回收期限（≤1 个版本），CI 检出非空 patch 时在日志显著告警；
  - 下一版本必须把紧急补丁转正为 fork 提交并清空 patch（发布核对清单加项）。
- 供应链审计：
  - 双侧钉死：wancode lock（repo+commit）+ fork 分支保护 + release 标签，任何构建可追溯到唯一引擎树；
  - 每次上游同步产出审计报告存 `docs/evidence/engine-sync/<date>.md`：上游区间 `git log --oneline old..new`、`git range-diff` 我方提交漂移、新增依赖清单（Cargo.lock diff 的 crate 级摘要）、许可证变化检查；
  - 发布证据（合规摘要 + compatibility.md）已记录 engine commit，形成"发布 ↔ 引擎树"的双向可验证链；
  - fork 仓库最小权限：仅维护者可写 integration/mirror；上游 tarball 不入链（一切以 git commit 为源）。

## 6. 评审后的执行顺序（预估）

B1（1 PR，小）→ CI 断言三道（同 PR）→ B2/B5（身份+测试，最大块）→ B3 → B4 → 收尾清空。每批照既定纪律：Draft PR + 外部复核 + 干净 CI。全程 lock 可发布。

## 7. 开放问题（请评审裁决）

1. B5 引擎测试是否与 B2 合并为一批（同域强耦合）？
2. 同步审计报告是否要求对上游每个 commit 逐条标注采纳/跳过，还是区间级摘要即可？
3. fork 上是否为 wancode 每个 release 打标签（§2），或仅在引擎 commit 变化时打？
