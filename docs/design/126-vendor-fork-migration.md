# #126 设计稿：vendor 补丁迁移自有 fork（v0.19 基础设施）· 修订二版

> 状态：**设计评审中，未开工迁移**。评审通过前不做任何迁移动作（含 B1）。
> 现状事实：引擎 = fork `ThomasWan123/grok-build` @ `b189869b7755d2b482969acf6c92da3ecfeffd36`；本地补丁 `vendor/grok-build-local.patch` **7153 行 / 30 文件**；bootstrap 的实际序列 = clone → `git apply patch` → **`Cargo.lock` 被 `vendor/grok-build-Cargo.lock` 覆盖**。

## 0. 核心定义：有效树（Effective Tree）

一切等价审计与 CI 断言的对象都是**有效树**，不是裸 fork 树：

```
有效树 = fork@lock.commit
         + wancode 侧接线 patch（迁移后=永久最小接线，见 §0.1）
         + Cargo.lock 覆盖（vendor/grok-build-Cargo.lock）
```

### 迁移前 / 迁移后有效树示意

```
迁移前                                    迁移后
─────────────────────────────            ─────────────────────────────
fork b189869b（上游基线，无我方提交）      fork <新commit>（基线 + 我方全部
        │                                 产品域提交，可 blame/回滚）
        ▼                                         │
git apply 7153 行 patch                           ▼
  ├─ 构建接线（workspace member、protoc）  git apply ≤50 行永久接线 patch
  ├─ 模型身份 + 其测试                       └─ 仅 workspace member 注入
  ├─ 图片转述管线 + 其测试                      （本机目录结构，见 §0.1）
  └─ 兼容杂项 + 其测试                            │
        │                                         ▼
        ▼                                 Cargo.lock 覆盖（不变）
Cargo.lock 覆盖                                   │
        │                                         ▼
        ▼                                    ＝ 有效树
   ＝ 有效树                       【迁移不变量：两侧有效树逐字节相等】
```

### 0.1 永不进 fork 的部分（防"本机目录结构污染"）

- `Cargo.toml` workspace members 注入 `"../wancode/src-tauri"`：这是**本机目录布局**（wancode 与引擎互为兄弟目录）的接线，进 fork 会把一个仓库外相对路径烧进引擎树、单独 clone fork 即坏。**永久保留在 wancode 侧接线 patch**（迁移终态 ≤50 行）。
- `vendor/grok-build-Cargo.lock` 覆盖：wancode 挂入后的完整依赖解析，属 wancode 构建产物空间，不进 fork。fork 侧改动 `Cargo.toml` 的批次必须在**同一 wancode PR** 内再生该覆盖文件（cargo 在有效树上重解析），审计含此再生结果。
- protoc/Windows 修复（`xai-proto-build`）：不含本机路径，**迁入 fork**（B1）。

## 1. fork 与上游同步策略

- 分支模型：
  - `upstream-mirror`：只读镜像上游 default 分支，仅 fast-forward，永不含我方提交；
  - `wancode-integration`：上游基线 + 我方提交；wancode lock 只允许指向此分支上的 commit；
  - 主题分支 `wc/<topic>` 经 fork PR 合入 integration（merge，不 rebase 已发布历史）。
- 同步节奏：**按需**，每次为显式工程事件：mirror FF → `sync/<date>` 分支 merge → 审计报告（§5）→ PR 评审合回 → wancode lock bump PR + 全量 CI + smoke。
- 审计颗粒度（评审裁决②）："**机器完整、人工聚焦**"——自动保存完整 `old..new` commit 列表入报告；人工只逐条标注**冲突、依赖、安全、协议与行为风险项**。仅当采取选择性 cherry-pick 时，才要求逐 commit 标注采纳/跳过。

## 2. commit 固定与回滚

- 固定：`vendor/grok-build.lock`（repo + 40 位 commit）唯一事实源，不引入浮动分支引用。
- 标签（评审裁决③）：**仅在 engine commit 变化时**在 fork 打 `wancode-engine/<engine-short-sha>` 标签；每个 WanCode release 继续在发布证据（合规摘要 + docs/evidence）记录 engine commit——同一引擎 commit 不堆积多个标签。
- 不可变性：integration/mirror 分支保护、禁 force-push；错误提交 revert 前进式修复。
- 回滚：应用层 = revert wancode 的 lock bump（一步）；引擎层 = integration revert + 新 lock bump；迁移期 = 每批独立，退回上一批 lock commit 即完全恢复，无半迁移状态。

## 3. 7153 行 / 30 文件的分批迁移（评审裁决①：测试与产品域同批）

> 纪律：每批 = fork 主题分支 → fork PR（逐 hunk 评审）→ 合入 integration → wancode 同一 PR 内：裁剪 patch + 再生 Cargo.lock 覆盖（若 Cargo.toml 变）+ lock bump → **全树有效树等价审计** → 全量 CI + smoke 6/6。任何一批红即停。

| 批 | 域（产品 + **其测试同批**） | 独立验收 |
|---|---|---|
| B1 | 构建接线中可迁部分：protoc/Windows 修复（workspace member 注入**永不迁**，留接线 patch） | wancode 全量构建 + CI 三 job + CI 新增三道断言（§4）落地 |
| B2 | 模型身份全域：catalog_model_id、resolve_override_ungated、modelBlock/ACP meta、双端点路由 **+ model_endpoint_routing.rs、model_identity_e2e.rs** | Gate 1 + 身份链 + model_block_over_acp 全绿；矩阵中 Gate 1 落点由"patch 引入"改 fork blob 直链 |
| B3 | 图片转述全域：transcribe 管线、尺寸门、降级垫底 **+ 其引擎侧测试** | 4b 套件 + 图片专项冒烟 |
| B4 | 兼容杂项全域：429 文案、object 宽容化、4v-flash max_tokens 等 **+ 其单测** | 4a 套件（错误组）+ 对应单测 |
| 收尾 | patch 缩至永久接线（≤50 行）；bootstrap/CI 断言切换到终态形状；文档落点全面改 fork blob | 全树有效树等价审计（终态）|

### 有效树等价审计（每批必做，全树、非按批范围）

比较对象消歧（本修订版核心）：

```
树 A = bootstrap(fork@新commit, 新裁剪patch, 新Cargo.lock覆盖)   ← 迁移后有效树
树 B = bootstrap(fork@旧commit, 旧patch,     旧Cargo.lock覆盖)   ← 迁移前有效树
断言：git diff --no-index 树A 树B == 空（全树，含 Cargo.lock）
```

- 审计脚本入仓（`scripts/audit_effective_tree.ps1`），两侧都走**同一 bootstrap 代码路径**产树，不手工拼装；
- **全树对比，每批执行**——不做"仅该批范围"的局部对比（局部范围定义模糊，正是"审计说等价、实际对象不等价"的来源）；
- 唯一允许的差异白名单：无（Cargo.lock 若因 fork Cargo.toml 变更而再生，再生结果本身就是新覆盖文件，两树各用各的覆盖后仍须逐字节相等——不相等即该批引入了行为变化，红停）。

## 4. CI 如何证明使用了指定 fork commit（B1 随批落地）

1. clone 步后断言：`git -C $engine rev-parse HEAD == lock.commit`，否则 fail；
2. 接线 patch 硬断言：CI 记录并校验 patch 文件 sha256 与仓库内当前版本一致；`git -C $engine status --porcelain` 只含预期改动（patch 触及文件 + Cargo.lock 覆盖）；
3. `engine_commit` 字段写入合规摘要（COMPLIANCE_SUMMARY）与发布证据，与 compatibility.md 落点闭环。

## 5. 紧急补丁通道与供应链审计

- 紧急通道**硬契约**（评审升级）：接线 patch 之外的紧急内容非空时，patch 头部必须含三要素——**事故编号、到期版本、patch 内容 hash**；CI 解析头部：
  - 缺任一要素 → **fail**（不是告警）；
  - 当前版本 ≥ 到期版本仍未清空 → **fail**；
  - hash 与实际内容不符 → **fail**。
  - 到期前每轮 CI 显著打印剩余期限。
- 供应链审计：
  - 双侧钉死（lock + 分支保护 + 变更即打标签）；任何构建可追溯唯一有效树；
  - 同步审计报告存 `docs/evidence/engine-sync/<date>.md`：完整 commit 列表（机器）+ 风险项人工标注（§1）+ Cargo.lock diff 的 crate 级新增/升级摘要 + 许可证变化检查；
  - 发布证据记录 engine commit（已有），与 fork 标签互指；
  - fork 最小写权限；上游 tarball 不入链，一切以 git commit 为源。

## 6. 评审后的执行顺序

B1（含 CI 三断言 + 审计脚本入仓）→ B2 → B3 → B4 → 收尾。每批 Draft PR + 外部复核 + 干净 CI + 全树等价审计；全程 lock 可发布、任一批可退。

## 7. 已裁决与遗留

- ✅ 裁决①：取消独立 B5，测试随产品域同批（B2/B3/B4 已并入）。
- ✅ 裁决②：同步审计"机器完整、人工聚焦"。
- ✅ 裁决③：仅 engine commit 变化时打标签。
- ✅ 紧急 patch 升级为硬契约（三要素 + 到期即 fail）。
- 开放：无（本版无新增开放问题；承重修订见 §0——有效树定义、workspace member 永不迁、全树审计）。
