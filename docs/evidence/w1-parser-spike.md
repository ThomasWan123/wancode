# W1 解析可行性 spike — 证据报告

> 对照 `docs/design/v0.20-work-cowork-increment.md` §1.1 W1 双门清单的**安全面**。
> spike 代码:`spike/w1-parser/`(独立 workspace,不进产品构建)。
> 结构化证据:`w1-parser-spike.json`。

## 头号发现:纯 Rust 路线可行,原生二进制风险消解

设计稿 §1.1 首选 pdfium-render(原生 PDFium),§5 记为最大风险(~5MB 原生
二进制 + 崩溃遏制 + 架构矩阵)。spike 实证**纯 Rust 栈(lopdf + zip +
docx-rs)在安全面全部达标,零原生依赖**:

- `native_binary: false` — 无 PDFium/DLL 分发,打包门体积零增量风险;
- 顺带发现:锁定引擎自身已内置 `pdf_oxide 0.3.43`(纯 Rust),侧证纯 Rust
  PDF 解析在本项目技术栈内是成熟选择。

**建议修订设计稿选型**:PDF 首选从 pdfium-render 改为纯 Rust(lopdf 做结构/
文本抽取,或复用引擎的 pdf_oxide 评估),pdfium 降为"若纯 Rust 抽取率不足"
的备选。此修订待 W1 功能面(抽取率)spike 补齐后定稿。

## 安全面探针 6/6(all_safe=true,退出码 0)

| 探针 | 结局 | 说明 |
|---|---|---|
| pdf_truncated | REJECTED | 截断 PDF(无 xref/trailer)→ 结构化错误,无 panic |
| pdf_garbage | REJECTED | 4KB 垃圾字节 → Invalid file header,拒绝 |
| pdf_valid_control | OK | **正对照**:合法单页 PDF 解析成功(页数=1)——证明拒绝不是"全拒" |
| docx_zip_path_traversal | REJECTED | `../../evil.txt` 条目 → `enclosed_name()` 返回 None(安全 API 拦截逃逸) |
| docx_zip_bomb_guard | OK | 解压**前**可读声明大小 → 上限判定机制成立(CAP=200MB) |
| crash_containment | CONTAINED | 解析 panic 被 catch_unwind 兜住;生产须跑在可杀工作进程 |

## 尚未覆盖(W1 功能面,下一 spike / W3 前置)

- 抽取率:真实样本(中文/表格/多栏)的文本抽取质量与锚点回源逐字一致;
- 加密 PDF、扫描件(无文本层→「无法定位」);
- DOCX 段落/块级锚点(docx-rs 段落树遍历)与 run 拆分;
- 内存/CPU 上限的实测数值定档(本 spike 只验机制存在,未压测)。

## 结论

W1 安全门的**机制侧全部就绪且纯 Rust**。设计稿 §5 的原生二进制体积风险
按此证据可从风险表移除或大幅降级。功能面 spike 通过后即可定稿选型、开 W2。
