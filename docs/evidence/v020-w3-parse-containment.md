# W3-P2 解析崩溃遏制外壳 · 实证

复现：

```
cargo test --locked -p wancode --test work_parse_containment
```

对应设计 §1.1 安全面「崩溃遏制」：解析跑在独立工作进程（超时可杀），
Pdfium 原生崩溃/panic 不得带倒主应用；失败即整体拒收，暂存区无半成品。

**本 PR 只做外壳，不含解析器。** 拆开是为了让「隔离是否成立」能被单独证伪，
不和解析器的正确性搅在一起。

## 「暂存区无半成品」靠什么成立

不靠 worker 自觉清理——崩溃的进程没有自觉。靠**它根本没有写入路径**：
worker 从 stdin 收请求、往 stdout 吐响应，全程不碰暂存区，也不接收暂存区路径
（`ParseRequest` 只有 `kind` + `source_path`，即原件只读路径）。父进程拿到一份
完整且合法的响应后才自己写盘。worker 死在任何一步，暂存区都不曾被触碰——
零残留是结构性的，不是清理出来的。

## 进程树治理

W1 spike 用的是裸 `Child::kill()`。Windows 上那只杀直接子进程，孙进程变孤儿——
作为可行性探针够用，作为产品代码不够。这里每次调用建一个独立 Job
（`KILL_ON_JOB_CLOSE` + `PROCESS_MEMORY`），超时用 `TerminateJobObject` 整树清杀，
等待结束后关句柄（不关就是每次解析泄一个句柄）。

不加 `BREAKAWAY_OK`：解析 worker 必须随应用一起死，没有任何正当理由脱离
（与更新安装器相反，那个必须活过应用退出）。

## 结果：R1 时 8/8，R2 追加一条后 **9/9**

| 断言 | 结果 |
| --- | --- |
| `positive_control_echo` | Ok — **正对照**，没有它「全判失败」会看起来全绿 |
| `no_parser_yet_is_orderly_rejection` | `Rejected` — 证明 dispatch 真走到了，不是崩在半路 |
| `abort_is_contained_as_crashed` | `Crashed { code: -1073740791 }`（0xC0000409），父进程存活 |
| `hang_is_killed_at_deadline` | `Timeout{2s}`，用时 2.007s |
| `non_json_output_is_bad_output` | `BadOutput` — 正常退出但协议被破坏，不得当成功 |
| `output_flood_is_capped_not_deadlocked` | `OutputTooLarge`，用时 **15.9ms** |
| `oversize_input_rejected_before_spawn` | `InputTooLarge` — 根本不起进程 |
| `missing_source_is_unreadable` | `SourceUnreadable` |

`CONTAINMENT DONE pass=8 fail=0`（R1）→ `pass=9 fail=0`（R2，见下）

## 这套测试抓到的真 bug（写它就是为了抓这个）

第一版 `read_capped` 超限后仍然「礼貌地把管道读干」——理由是不读会让 worker
阻塞在 write 上造成死锁。结果是 worker 可以无限吐、父进程一直陪到墙钟：

```
FAIL output_flood_is_capped_not_deadlocked — Err(Timeout { after: 60s }) 用时=60.0139508s
```

报的是 `Timeout` 而不是 `OutputTooLarge`——**输出上限形同虚设**，真正兜住的是
墙钟。修法：超限是**终止理由**，不是继续读的理由。读取线程置原子标志即返回，
等待循环看到标志立刻整树清杀。既不死锁，也不陪跑：

```
PASS output_flood_is_capped_not_deadlocked — Err(OutputTooLarge { cap: 4194304 }) 用时=15.8644ms
```

**60.01s → 15.9ms。** 记这一条是因为方法论比结论重要：如果这条断言只写
`is_err()`，它从头到尾都会显示通过，而上限一直是坏的。断言**失败的种类**和
**用时上界**才抓得到。同理，`hang` 那条同时断言 `≥2s`——只断言 `is_err()` 的话，
一个「立刻返回错误」的 bug 也会通过，而那恰恰说明超时机制没跑。

## R1 后追加（z-code #56 复核）

**P1 — 这套测试在 CI 里从未运行。** `ci.yml` 的 rust job 是**白名单显式列目标**，
而 `work_parse_containment` 不在其中。也就是说：rust ✅ 49m8s 的绿里**不含这
8 条断言**。我核了 `ci.yml:131` 与 run `31933312549` 的日志，finding 属实。

这条特别值得记：本 PR 的全部安全承诺都压在这套对抗测试上，而它不会被自动
执行——**一个不会被自动执行的对抗测试等于没有对抗测试**。已把
`--test work_parse_containment` 加进 CI 白名单。

**P2-1 — Job 失败原本静默降级。** 建议是「至少打日志」，实际改得更硬：
拿不到 Job 就**拒绝解析**（`ContainmentUnavailable`）。理由是 `eprintln!` 在
GUI 进程里无人可见，而失去整树清杀意味着设计 §1.1 的「超时可杀」不再成立；
对不受信文档降级运行，等于把遏制承诺悄悄变成尽力而为。

同时加了注入点 `WANCODE_PARSE_WORKER_SELFTEST=nojob` 让这条分支**可被证伪**
——没有注入点，`ContainmentUnavailable` 就是一条永远跑不到、无人验证的死代码。
新增断言 `no_job_means_refuse_not_degrade`，本地 **9/9**。

## NOT-RUN / 不在范围内

- **解析器未接入**：`run_request` 目前对产品路径一律有序拒收。DOCX/PDF 解析、
  以及解压后体积/页数/块数上限，随下一个 PR 进。
- **资源边界未实测定档**：`ParseLimits::default()` 的四个数字（64MB 输入 /
  30s 墙钟 / 512MB 进程内存 / 32MB 输出）是**保守起点，不是实测结果**。
  设计 §1.1 要求「资源边界实测并定档」，实测随解析器接入那一 PR 做。
- **内存上限未做行为验证**：Job 的 `ProcessMemoryLimit` 已设，但没有写一个
  「故意分配超限」的探针去证明它真的会拦。本轮不声称它已验证。
- **P2-2 孙进程逃逸未覆盖**（z-code 提出）：`Ended::Exited` 只保证直接子进程
  退出，孙进程靠关 Job 句柄时的 `KILL_ON_JOB_CLOSE` 清掉。逻辑对，但**没有
  测试**——要造一个真的起孙进程的 worker 才能证。本轮不声称已验证。
- **P2-3 stderr 路径不对称**（z-code 提出）：stdout 超限即杀，stderr 超限仍读干。
  worker 若往 stderr 无限写，那条读取线程会陪到墙钟。影响小（stderr 只是诊断，
  且墙钟仍兜底），但机制确实不对称，本轮**未改**。
- **本机构建注记**（与被测代码无关）：本机并行链接会争抢 `wancode_lib.dll`
  报 LNK1104，需 `-j 1`。已确认是**环境问题不是本 PR 引入**——已有的
  `job_breakaway` 测试目标同样复现。
