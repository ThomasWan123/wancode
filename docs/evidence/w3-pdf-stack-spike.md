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
| 志愿表：是否抽出 | **是**（抽取率 1.0） | **panic** `unsupported encoding UniGB-UCS2-H` | 名义 1.0，中文字 0，乱码率 0.984 |
| 逐页/几何 API（锚点需要） | 有页，文本弱 | **有**：`OutputDev::begin_page(page_num, media_box, art_box)` + 带位置的文本输出 | 有逐页 |

## 结论

**杀死条件未触发**：裁断定义的杀死条件是「Type0/CID 样本上两者抽取率都为 0」。
`pdf-extract` 在该样本上抽出 838 个中文字、乱码率 0，因此**选项 2 按字面通过**。

但必须同时记录三条限制，通过**不等于**选型落地：

1. **没有任何单一纯 Rust crate 覆盖两个样本**。`pdf-extract` 在 CID+ToUnicode 件上成功、在传统 CMap 件上 **panic**；`lopdf` 恰好相反；`pdf` crate 两个都拿不到中文字符。
2. **`pdf-extract` 的失败是 panic 不是错误返回**（`pdf-extract-0.9.0/src/lib.rs:983` 无条件 `panic!("unsupported encoding {}", name)`）。`UniGB-UCS2-H` 是简体中文 PDF 的标准 Adobe CMap，出现频率不低。生产使用需要 panic 遏制或上游/补丁支持预定义 CMap。
3. **锚点定位子可行性：正面**。`pdf-extract` 的 `OutputDev` 提供 `begin_page(page_num, media_box, art_box)` 与带位置的文本输出，故锚点契约要求的 `page` 与几何（bbox）可得；`chunk` 与 `raw_range` 由我们在逐页文本流上自建。整篇 `extract_text` 不足以支撑锚点，但该 crate 的 API 形状**能**支撑。

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
