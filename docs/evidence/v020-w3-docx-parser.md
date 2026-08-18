# W3 DOCX 解析器进产品代码 · 实证

```
cargo test -p wancode --lib work_docx
cargo test -p wancode --test work_parse_containment
```

真实样本探针（样本**不入库**，个人文档；未设环境变量即跳过，故 CI 上永远
NOT-RUN）：

```
WANCODE_DOCX_SAMPLE="<path>\某文档.docx" cargo test -p wancode --lib real_sample -- --nocapture
WANCODE_DOCX_SAMPLE="<path>\某文档.docx" cargo test -p wancode --test work_parse_containment
```

## 相对 W3 spike 补上的事

spike 证的是「能不能抽出来」，产品代码还得证「抽不出来时会不会安静地出错」。

**① run 之外的文本会在 `runs` 里留缺口。** spike 无条件把 `<w:t>` 文本追加进
`raw`，不管它在不在某个 `<w:r>` 里。而 `WorkBlock::is_well_formed()` 要求 runs
**铺满** `[0, len)`——缺口意味着落在缺口里的区间会算出**空的 `run_ordinals`**，
等于铸出一个指不到任何 run 的锚点。产品代码改为只在 run 内累计文本，并统计
被丢弃的字符；**丢过就整篇拒收**（`TextOutsideRun`）。既不静默丢正文，也不
产出破的 tiling。

**② DOCTYPE 一律拒。** 实体展开炸弹的前提是 DTD。W1 把这条列为 REJECTED，
但那是 spike 里的独立探针——产品读取路径必须自己拦，不能靠"W1 证过了"。

**③ zip 炸弹两道闸。** 解压前先看声明的解压后体积；解压时再按上限截断读取
（读 `cap+1` 字节，读满即超限）。第二道是必需的：声明值是攻击者可控的。

**④ 按命名空间 URI 认元素（#57 R1-P1）。** 字面匹配 `w:p`/`w:r`/`w:t` 会把
前缀别名或默认 xmlns 的合法 WordprocessingML 抽成 `Ok([])`，worker 再序列化成
空 `ParsedDoc::Docx`。改为 `NsReader` 按
`http://schemas.openxmlformats.org/wordprocessingml/2006/main` 的 local name
认 `p`/`r`/`t`；`w:t` 内 CDATA 当正文；从未见过 Word NS 元素则
`UnrecognizedWordprocessing`，绝不把「没认出来」当成空文档。

**⑤ 嵌套段落不得覆盖外层 `cur`（#65 R1-P1）。** 外层段已抽出的正文在遇到
内层 `<w:p>` 时会被 `cur = Some(...)` 丢掉，成功返回只剩内层。改为开始新
段前先收口当前段；内层结束后若还在外层里，再开一块承接段尾。夹具
`nested_paragraph_does_not_drop_surrounding_text`：甲+乙 与 甲+乙+丙。

另外，**路径穿越在这里结构性不适用**：我们从不落盘，只按精确名
`word/document.xml` 取一个条目读进内存。没有写路径，`../` 无处施展。

## 单元断言 13/13

| 断言 | 意图 |
| --- | --- |
| `two_runs_tile_the_paragraph` | **正对照**——没有它，「一律拒收」的实现会让所有负例都通过 |
| `empty_run_is_skipped_without_creating_a_gap` | 零宽 run 不记（记了破坏"每条非空"），但不得造成缺口 |
| `text_outside_run_is_rejected_not_silently_dropped` | 拒收而非静默丢弃，`dropped_chars=3` |
| `doctype_is_rejected` | 实体炸弹前提 |
| `blank_paragraph_skipped_but_index_advances` | 空段不产块，但**段号照进**——否则锚点指错段 |
| `block_cap_is_enforced` | 块数上限 |
| `surrogate_pair_counts_as_two_utf16_units` | emoji = 2 个 UTF-16 单元；按 char 计会整体错位 |
| `alternate_prefix_bound_to_word_ns_is_parsed` | `word:` 绑到标准 Word NS 必须抽出正文，不能 `Ok([])` |
| `default_xmlns_word_ns_is_parsed` | 默认 xmlns、无前缀的合法结构必须抽出 |
| `cdata_inside_t_is_text_not_dropped` | `w:t` 内 CDATA 是正文 |
| `no_word_namespace_is_rejected_not_empty_ok` | 非 Word XML → `UnrecognizedWordprocessing`，禁止 `Ok([])` |
| `wrong_namespace_on_w_prefix_is_rejected` | 字面 `w:*` 绑错 NS → 拒收，禁止误抽 |
| `nested_paragraph_does_not_drop_surrounding_text` | 嵌套 `<w:p>` 不得丢掉外层段首/段尾；旧代码 Ok 只剩「乙」 |

RED-first：`nested_paragraph_does_not_drop_surrounding_text` 在覆盖 `cur` 的旧代码上 **failed**（`Ok` 得 `"乙"`）。改段栈后 13/13。

全量：`cargo test -p wancode --lib work_docx` **15 passed**（含 2 条未设样本即 SKIP 的真实探针）。clippy `-D warnings` exit 0。
本轮未重跑 `work_parse_containment`（沙箱 target 把 `panic=abort` 的 dev
产物和 `panic=unwind` 的 test 产物混在一起，编不过；与本次解析器改动
无关）。worker 仍只调用 `parse_docx`，嵌套段回归在 `work_docx` 单元里。

## 真实样本（本地，非 CI）

一份中文 .docx：

```
REAL DOCX：块=63 总UTF16=1768 run总数=82 中文字=1117 平均run/块=1.30
REAL DOCX 锚点：铸造=189 跨独立再解析取回成功=189
```

**189/189 逐字回源**，且每一块 `is_well_formed()`。

关键在「跨独立再解析」：锚点在**第一次**解析上铸造，对**独立第二次**解析的
结果取回比对。拿同一次结果自比是恒等式，证不了任何东西——本仓在早期的锚点
测试上正栽过这个跟头。

## 端到端

`work_parse_containment` 加了第 9 条：真实 DOCX 走**完整 worker 路径**
（进程隔离 → 解析 → 协议往返），得 63 块且全部 well-formed。

- 设了样本：`CONTAINMENT DONE pass=9 fail=0`
- 未设样本：`SKIP …` + `CONTAINMENT DONE pass=8 fail=0` ← CI 上的形态，已实测

## NOT-RUN / 不在范围内

- **PDF 解析器未接入**：`DocKind::Pdf` 返回有序拒收「PDF 解析器尚未接入」。
- **资源边界仍未实测定档**：`DocxLimits::default()`（64MB / 20 万块 / 单块 100 万
  UTF-16）与 `ParseLimits::default()` 同样是**保守起点**。设计 §1.1 要求实测
  定档，仍未做——上一个 PR 记的这条账本轮**没有还**。
- **zip 炸弹未用真炸弹验证**：两道闸的代码在，但没有构造一个真实的高压缩比
  样本去证明它们会拦。本轮不声称已验证。
- **只测了一份真实样本**：设计要求「三份真实样本（扫描件/中文/表格）」。
  本轮只过了中文那份；表格与扫描件属 PDF 面，随 PDF 接入一并做。
- **`w:tab` / `w:br` 等不产文本的元素**：当前不产生任何字符，因此不影响
  tiling。但这意味着抽取文本里**没有制表/换行信息**——对锚点无害，对将来
  的只读查看器排版还原有影响，届时需单独设计。
- **嵌套段被拆成相邻块**：文本框里的 `<w:p>` 会先收口外层片段再作为独立块，
  不再静默丢字；不还原绘图/文本框布局。
- **前 head `2a7a71b0ecada1a163047df363c1994f9b3a934c`** 三项 required checks
  已绿（Actions run `32037071895`），base 即当时 `main` `fef7eba`。本文件此
  前仍写「base 不是 main、不能 ACCEPT」，与事实不符（#65 R1-P2）。新 head
  的 CI 见 PR 证据表。
