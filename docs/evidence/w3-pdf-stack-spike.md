# W3 PDF 解析栈证据 spike（选项 2：纯 Rust 候选并列，唯一一轮）

> **范围**：按裁断执行的**一次且仅一次**选项 2 证据 spike。无产品代码。
> `pdf-extract` 与 `pdf` crate 并列，同一对样本、同一套指标。
> **样本不入库**（真实私有文档，仅本地读取）；本文件只含指标，不含正文内容。

## 复现方式

```
cd spike/w3-parser-functional
cargo run --release --bin w3_opt2  -- <sample.pdf> <label>   # 选项 2 并列评测
cargo run --release --bin w3_pdf_probe -- <sample.pdf> <label>  # lopdf 基线（前一轮）
cargo run --release --bin w3_diag  -- <sample.pdf>            # 文字层/图像/字体普查
cargo run --release --bin w3_diag2 -- <sample.pdf>            # 字体类型 + ToUnicode 普查
```

样本（本地路径，未提交）：

| 标签 | 特征（由 `w3_diag2` 实测） |
|---|---|
| `英语词汇(CID+ToUnicode)` | 18 页；153 个字体引用**全为 Type0/CID**，**全部带 ToUnicode**；18 页均有图像 XObject，3 页有文本算子（878 个） |
| `志愿表(正对照)` | 1 页；3 个字体引用（1 简单 + 2 Type0），1 个带 ToUnicode |

## 结果

| 指标 | lopdf（基线） | **pdf-extract** | **pdf** crate |
|---|---|---|---|
| 英语词汇：是否抽出 | 否（抽取率 0.0） | **是** | 名义是，实为未解码字节 |
| 英语词汇：中文字数 | 0 | **838** | **0** |
| 英语词汇：乱码率 | — | **0.0** | 0.541 |
| 英语词汇：文本量(UTF-16) | 0 | 3712 | 4808（无效） |
| 志愿表：是否抽出 | **是**（抽取率 1.0） | **panic**（见下方 payload） | 名义 1.0，中文字 0，乱码率 0.984 |
| 逐页/几何 API（锚点需要） | 有页，文本弱 | 见「未 RUN 的读源码结论」 | 有逐页 |

### panic payload（可复现，codex #50 R1-P1）

`w3_opt2` 在志愿表上的 JSON 原样输出：

```json
"pdf_extract": { "ok": false, "panicked": true, "error": "unsupported encoding UniGB-UCS2-H", "ms": 28 }
```

即 `pdf-extract` 对该 CMap 是**无条件 panic**（`pdf-extract-0.9.0/src/lib.rs:983`
`panic!("unsupported encoding {}", name)`），不是错误返回。首版把 payload
收成一句 `"panic in pdf-extract"`，导致这条负载事实**用提交的命令复现不出来**；
现版 downcast `&str`/`String` 后写进 JSON。

### 「抽到了」的判据（codex #50 R1-P2）

`either_extracted_usable` = **非空 且 乱码率 < 0.1**。旧口径只看非空，会把
`pdf` crate 的未解码字节（乱码率 0.541 / 0.984）算成「抽到了」。按新判据：

| 样本 | either_extracted_usable |
|---|---|
| 英语词汇(CID+ToUnicode) | **true**（仅 `pdf-extract` 贡献） |
| 志愿表(UniGB-UCS2-H) | **false**（`pdf-extract` panic，`pdf` crate 全乱码） |

`junk_ratio` 只计 U+FFFD / NUL / 控制符，**不计** ASCII 问号（问号在正常文本里
合法，计入会误伤）——注释已与实现对齐。

## 结论

**杀死条件未触发**：裁断定义的杀死条件是「Type0/CID 样本上两者抽取率都为 0」。
`pdf-extract` 在该样本上抽出 838 个中文字、乱码率 0，因此**选项 2 按字面通过**。

但必须同时记录三条限制，通过**不等于**选型落地：

1. **没有任何单一纯 Rust crate 覆盖两个样本**。`pdf-extract` 在 CID+ToUnicode 件上成功、在传统 CMap 件上 **panic**；`lopdf` 恰好相反；`pdf` crate 两个都拿不到中文字符。
2. **`pdf-extract` 的失败是 panic 不是错误返回**（`pdf-extract-0.9.0/src/lib.rs:983` 无条件 `panic!("unsupported encoding {}", name)`）。`UniGB-UCS2-H` 是简体中文 PDF 的标准 Adobe CMap，出现频率不低。生产使用需要 panic 遏制或上游/补丁支持预定义 CMap。
3. **锚点定位子可行性：读源码结论，NOT-RUN**。`pdf-extract` 声明了
   `pub trait OutputDev { fn begin_page(&mut self, page_num: u32, media_box: &MediaBox,
   art_box: Option<(f64,f64,f64,f64)>) -> Result<(), OutputError>; ... }`
   （`lib.rs:1876-1878`），形状上能给出 `page` 与几何。**但本轮没有实际用
   `OutputDev` 跑出逐页 + bbox 数据**，故此条是**读源码**而非 RUN，不得当作
   已验证能力。整篇 `extract_text` 确定不足以支撑锚点（本轮 RUN 证实只有整篇粒度）。

## NOT-RUN / 未决

- 未评估 `pdf-extract` 加装预定义 CMap（含 `UniGB-*`）的工作量；
- 未做第三个纯 Rust crate（裁断禁止串行逛 crate）；
- 未跑 pdfium 对照（属选项 1，需单独授权）；
- 抽取**质量**（阅读顺序、表格结构）未评估，本轮只判「能否拿到正确字符」。

## 待裁

`pdf-extract` 是唯一在中文 CID 件上产出正确文本的纯 Rust 候选，且 API 形状满足锚点需要，
但对传统 CMap 会 panic。是否
(a) 采纳 `pdf-extract` 并投入 CMap 补齐 / panic 遏制，
(b) 转选项 1（pdfium-render，需单独授权原生二进制 + vendor-lock），
(c) 其他，
由评审与用户裁定。本 spike 不做选型落地。

## crate 版本（声称绑定）

| crate | 版本 | 用途 |
|---|---|---|
| `lopdf` | 0.34 | 基线（W1 同款） |
| `pdf-extract` | 0.9.0 | 选项 2 候选一 |
| `pdf` | 0.9 | 选项 2 候选二 |
| `zip`（仅 deflate） | 2.x | DOCX 面 |
| `quick-xml` | 0.37 | DOCX 面 |

三者**只存在于 `spike/w3-parser-functional`（独立 workspace + 自带 lock）**，
未进产品 `src-tauri/Cargo.toml`，未触碰 `vendor/grok-build-Cargo.lock`
（`scripts/audit_effective_tree.ps1 verify` 报 VERIFY OK）。

## DOCX 面（同一 spike，另一轮）

| 指标 | 结果（真实中文 .docx） |
|---|---|
| 块 / run | 63 段 / 82 run，其中 9 段跨多 run |
| 中文 | 可读，无乱码 |
| 锚点回源逐字一致 | run 锚点 **82/82**；跨-run 整段锚点 **9/9** |
| 重解析确定性 | true |
| 栈 | 纯 Rust（`zip` deflate + `quick-xml`），无 `*-sys` |

命令：`cargo run --release --bin w3_docx_probe -- <sample.docx>`

**方法论订正（记录在案）**：首版把「回源一致」写成同一函数同一输入算两遍再比较，
那是恒等式、不构成证据；现版**跨一次独立重解析**比对，同时覆盖解析确定性。
诊断脚本同样订正过一次：首版忽略 PDF **继承资源**，得出「0 字体 0 图像」的
自相矛盾结论，修正后才拿到真实根因（153 个 Type0/CID 全带 ToUnicode）。
