# dependency-delta 审计模式 · 实证

复现命令（PowerShell 5.1，与 CI 同一解释器）：

```
$before = (git rev-parse origin/main).Trim()
powershell -File scripts/migration_audit.ps1 -Mode dependency-delta -BeforeSha $before -OutFile dd.json
```

## 为什么要加第四种模式

本 PR 只做一件事：给 wancode 加解析依赖。引擎 commit 不动
（`2f480062e`），有效树里**只有 `Cargo.lock` 变**。三种既有模式都表达不了：

| 模式 | 断言 | 与本 PR 的关系 |
| --- | --- | --- |
| `equivalent` | 有效树逐字节相等 | 加依赖必然改 `Cargo.lock` → 不可能通过 |
| `version-only` | V3：lock diff 只有 wancode 版本行 | 加依赖新增 `[[package]]` → V3 必 FAIL |
| `intentional-delta` | A1：引擎 commit **变**；A4：lock **不变** | 本 PR 恰好相反（commit 不变、lock 变）→ A1+A4 双 FAIL |

`intentional-delta` 的 A1/A4 见 `scripts/migration_audit.ps1:346`（A4）与同段 A1。
硬套任何一种都只能靠放宽断言蒙混，等于把门拆掉，因此新增 `dependency-delta`。

## 正向：七项全 PASS

`D1` 引擎 commit 不变 · `D2` 有效树只有 Cargo.lock 变 · `D3` 既有 package 无删除/降版 ·
`D4` 新增集合与申报清单逐字相等 · `D5` 无新增 `*-sys` · `D6` wiring 未变 ·
`D7` 树/lock 哈希已登记且 emergency 为空。

新增 10 个 package（直接依赖 3：`zip` `quick-xml` `pdfium-render`；
`unicode-normalization` 已在树内故不计入新增；其余 7 个为传递依赖），
既有 package 删除/降版 **0** 个。

`MIGRATION AUDIT OK：dependency-delta 七项全 PASS（新增 10 个 package，无 *-sys，无既有 package 变动）` / exit 0

## 反向对照：证明门会咬

只报 PASS 的门等于没有门，故跑了两个反例：

**A — 申报漏写一个（清单里删掉 `zip 2.4.2`）**

```
D4_added_matches_declared=FAIL — added=[… zip 2.4.2; zopfli 0.8.3] declared=[… vecmath 1.0.0; zopfli 0.8.3]
MIGRATION AUDIT FAIL：dependency-delta 有 1 项断言失败   exit 1
```

未申报的新增 package 无法混过去 → 「多出来什么」确实是评审对象。

**B — 伪造一个 `fake-sys 1.0.0` 并**如实申报**

```
D4_added_matches_declared=PASS   ← 申报是诚实的
D5_no_native_sys_added=FAIL — 新增原生链: fake-sys 1.0.0
MIGRATION AUDIT FAIL：dependency-delta 有 2 项断言失败   exit 1
```

关键点：**申报不能豁免 `*-sys`**。W1 的教训（原生链同时放大构建复杂度与
供应链面）此前只写在注释里，现在是机器强制的独立断言。
第 2 项失败是 `D7`——伪造 package 改了 lock 哈希，哈希登记门跟着咬，属预期的
纵深防御，不是异常。（z-code 在 #53 轮要求确认此点，已确认。）

两次反例跑完后 `vendor/` 已复原：`vendor/grok-build-Cargo.lock` 的 sha256
仍为 `f89e1d05…8adf81`，与清单 `cargo_lock_sha256` 一致。

## 限制（门抓不住什么）

**D5 是后缀启发式，不是原生依赖检测。** 它匹配的是包名以 `-sys` 结尾。
Rust 生态里这是强约定但不是强制——一个通过 `build.rs` 编译 C/C++、却**不带
该后缀**的 crate 能干干净净地过 D5。所以 D5 的准确表述是「挡住按约定命名的
原生绑定」，不是「挡住一切原生依赖」。

兜底是**结构性的、人审的**：D4 强制每个新增包按 `name version` 逐字进清单，
所以它必然出现在 diff 里、成为评审对象。也就是说这个缝**不会让新增依赖隐身**，
只是不会被机器自动判为违规——评审时得自己看一眼新增包里有没有 `build.rs`
编译原生代码的。

不把 D5 升级成真检测（比如查 `build.rs`/`links` 字段），是因为 `Cargo.lock`
里没有这些信息，要判就得下载并解析每个 crate 的 `Cargo.toml`，那会把一个
离线、确定性的审计变成联网操作——代价大于收益。**这条记在这里，是为了让
下一个人知道缝在哪，而不是以为 D5 是完备的。**

（本条由 Cursor 在 #54 轮评审中作为「限制、非 finding」提出，我认下并记录。）

## NOT-RUN

- 本模式在 **CI** 上的首跑结果：本文件写作时尚未回。CI 结论以 PR 证据表
  绑定的那次 run 为准，不以本地跑替代。
