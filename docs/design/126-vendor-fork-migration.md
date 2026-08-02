# #126 设计稿：vendor 补丁迁移自有 fork（v0.19 基础设施）· 修订四版

> 状态：**设计评审中，未开工迁移**。评审通过前不做任何迁移动作（含 B1）。
> 现状事实：引擎 = fork `ThomasWan123/grok-build` @ `b189869b7755d2b482969acf6c92da3ecfeffd36`；本地补丁 `vendor/grok-build-local.patch` **7153 行 / 30 文件**（迁移中拆为 wiring/emergency 双文件，见 §5）；bootstrap 的实际序列 = clone → `git apply patch` → **`Cargo.lock` 被 `vendor/grok-build-Cargo.lock` 覆盖**。

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

- `Cargo.toml` workspace members 注入 `"../wancode/src-tauri"`：这是**本机目录布局**（wancode 与引擎互为兄弟目录）的接线，进 fork 会把一个仓库外相对路径烧进引擎树、单独 clone fork 即坏。**永久保留在 `vendor/grok-build-wiring.patch`**（迁移终态 ≤50 行，与紧急补丁物理分离，见 §5）。
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

- 固定：`vendor/grok-build.lock` **升级为构建清单（build manifest）**——不只钉 fork 分量，而是登记有效树全部输入与预期产物的内容哈希，成为**可校验清单**：

  ```
  repo=<fork url>
  commit=<40 位 sha>
  wiring_patch_sha256=<常驻接线 patch 内容哈希>
  emergency_patch_sha256=none | <紧急 patch 内容哈希>
  cargo_lock_sha256=<Cargo.lock 覆盖文件内容哈希>
  effective_tree_sha256=<有效树规范化摘要，见 §3 审计>
  ```

  "同一 wancode commit 可追溯三份输入"只解决溯源，不解决**校验**——清单哈希让 CI 能证明 patch/覆盖文件的字节内容与预期一致，`effective_tree_sha256` 让任何一方可独立复算整棵有效树。lock 不引入浮动分支引用。
- 标签（评审裁决③）：**仅在 engine commit 变化时**在 fork 打 `wancode-engine/<engine-short-sha>` 标签；每个 WanCode release 继续在发布证据（合规摘要 + docs/evidence）记录 engine commit——同一引擎 commit 不堆积多个标签。
- 不可变性：integration/mirror 分支保护、禁 force-push；错误提交 revert 前进式修复。
- 回滚：应用层 = **revert 整个 wancode 批次提交**（清单、接线 patch、紧急 patch、Cargo.lock 覆盖四项在同一提交内，一次 revert 全量恢复——不存在"只回 commit 不回 patch"的半恢复态）；引擎层 = integration revert + 新批次提交；迁移期 = 每批独立，revert 该批的 wancode 提交即完全恢复。

## 3. 7153 行 / 30 文件的分批迁移（评审裁决①：测试与产品域同批）

> 纪律：每批 = fork 主题分支 → fork PR（逐 hunk 评审）→ 合入 integration → wancode 同一 PR 内：裁剪 patch + 再生 Cargo.lock 覆盖（若 Cargo.toml 变）+ lock bump → **全树有效树等价审计** → 全量 CI + smoke 6/6。任何一批红即停。

| 批 | 域（产品 + **其测试同批**） | 独立验收 |
|---|---|---|
| B1 | 构建接线中可迁部分：protoc/Windows 修复（workspace member 注入**永不迁**，留接线 patch）；**bootstrap 参数化 + 审计脚本入仓**（审计可执行性前提） | wancode 全量构建 + CI 三 job + CI 三道断言（§4）落地 + 用审计脚本自证 B1 等价 |
| B2 | 模型身份全域：catalog_model_id、resolve_override_ungated、modelBlock/ACP meta、双端点路由 **+ model_endpoint_routing.rs、model_identity_e2e.rs** | Gate 1 + 身份链 + model_block_over_acp 全绿；矩阵中 Gate 1 落点由"patch 引入"改 fork blob 直链 |
| B3 | 图片转述全域：transcribe 管线、尺寸门、降级垫底 **+ 其引擎侧测试** | 4b 套件 + 图片专项冒烟 |
| B4 | 兼容杂项全域：429 文案、object 宽容化、4v-flash max_tokens 等 **+ 其单测** | 4a 套件（错误组）+ 对应单测 |
| 收尾 | patch 缩至永久接线（≤50 行）；bootstrap/CI 断言切换到终态形状；文档落点全面改 fork blob | 全树有效树等价审计（终态）|

### 有效树等价审计（每批必做，全树、非按批范围）

比较对象消歧：

```
树 A = bootstrap(fork@新commit, 新裁剪patch, 新Cargo.lock覆盖)   ← 迁移后有效树
树 B = bootstrap(fork@旧commit, 旧patch,     旧Cargo.lock覆盖)   ← 迁移前有效树
断言：两棵**工作树**（排除 .git）逐字节相等
```

可执行性前提（B1 必须先落地，否则审计无法按文执行）：

1. **bootstrap 参数化**：现行 `bootstrap.ps1` 写死兄弟目录 `../grok-build`，无法同机产两棵树。B1 第一项改动 = bootstrap 增加 `-Dest <dir>`（缺省保持现行为），审计脚本用两个临时目录各产一棵，仍是同一 bootstrap 代码路径。
2. **规范化对比语义**：两棵树 clone 自不同 commit，`.git` 永不相等——原稿"全树 diff"恒红不可执行。审计脚本先做**规范化清单**：排除 `.git/`、`target/` 与审计临时文件，对其余全部相对路径 + 文件字节计算哈希，得出排序清单；两树清单逐项相等即等价，清单整体再哈希即为 `effective_tree_sha256`（写入构建清单，供 CI 与第三方复算）。Cargo.lock 属工作树，照常入列。
3. 审计脚本 `scripts/audit_effective_tree.ps1` 与 bootstrap 参数化同批（B1）入仓，B1 自身即用它验收。

- **全树对比，每批执行**——不做"仅该批范围"的局部对比（范围定义模糊即假等价来源）；
- 差异白名单：无。fork Cargo.toml 变更的批次须同 PR 再生覆盖文件；两树各自套用各自覆盖后仍须逐字节相等，不相等即该批引入行为变化，红停。

## 4. CI 如何证明使用了指定 fork commit（B1 随批落地）

1. clone 步后断言：`git -C $engine rev-parse HEAD == lock.commit`，否则 fail；
2. **清单内容哈希断言**（P0 核心）：`wiring/emergency/cargo_lock` 三文件实际 sha256 各自 == 清单登记值；套用后按审计脚本规范化流程复算 `effective_tree_sha256` == 清单登记值——porcelain 精确集合断言（文件集合 == patch 触及清单 ∪ {Cargo.lock}）保留为快速结构检查，但**不替代内容哈希**；
3. `engine_commit` 字段写入合规摘要（COMPLIANCE_SUMMARY）与发布证据，与 compatibility.md 落点闭环。

## 5. 紧急补丁通道与供应链审计

- 紧急通道**硬契约 + 物理分离**（评审定案）：永久接线与紧急补丁**拆为两个文件**，不共存于同一 patch、不靠文本边界解析：
  - `vendor/grok-build-wiring.patch`：常驻（workspace member 注入等 ≤50 行），内容哈希登记于清单 `wiring_patch_sha256`，变更即改清单同提交；
  - `vendor/grok-build-emergency.patch`：**常态为空文件**（清单记 `emergency_patch_sha256=none`）；启用时头部三要素——**事故编号、到期版本、内容 hash**——并同步清单；
  - bootstrap 固定顺序：先 `git apply` wiring；emergency **仅在非空时**才 `git apply`（`git apply` 对空输入会直接报错 "unrecognized input"——空文件无条件套用会让 bootstrap 必然失败，空即跳过是执行语义的一部分，不是优化）；
  - CI 分别校验，`none` 的比较语义显式定义（空文件也有真实 sha256，"哈希 == none" 无法直接比较）：
    - 清单 `emergency_patch_sha256=none` ⇔ 断言 emergency 文件**存在且为 0 字节**（不比哈希）；
    - 清单为具体哈希 ⇔ 断言文件非空且 sha256 相等，且头部三要素齐备：缺任一要素 → **fail**、当前版本 ≥ 到期版本 → **fail**、头部自述 hash 与内容不符 → **fail**；
    - wiring 恒为非空哈希比较；到期前每轮 CI 显著打印剩余期限。
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
