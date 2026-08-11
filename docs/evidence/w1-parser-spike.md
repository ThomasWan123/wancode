# W1 解析可行性 spike — 证据报告(v2,已按 codex 复核收窄)

> 对照 `docs/design/v0.20-work-cowork-increment.md` §1.1 W1 安全面清单。
> spike:`spike/w1-parser/`(独立 workspace)。证据:`w1-parser-spike.json`(合法 JSON)。
>
> **范围声明(codex R1-F1)**:本 spike 是**安全面的部分证据**,**不**关闭/
> 计入 W1 安全门。数值资源上限(内存/CPU 墙钟精确档位)与真实样本抽取率
> **未覆盖**,归功能面/压测 spike。据此,PDF 选型修订(pdfium→纯 Rust)
> **不在本 PR 落地**,仅作为待功能面证据补齐后的建议记录。

## 依赖图:deflate-only 纯 Rust(codex R1-F4 已修)

初版 `zip = "2"` 默认特性拉进 `bzip2-sys`/`lzma-sys`/`zstd-sys` 原生链——
"零原生依赖"声明当时**造假**。已改 `zip = { default-features = false,
features = ["deflate"] }`(miniz_oxide 纯 Rust 后端);构建后核验 lockfile
**零 `*-sys` 压缩链**。精确声明:`native_binary: false` 现指"无单独分发的
原生二进制 **且** 压缩栈为纯 Rust deflate",不再宣称整图零原生。

## 安全面探针 8/8(all_safe=true;JSON 经 python json.tool 真解析)

| 探针 | 结局 | 说明 |
|---|---|---|
| pdf_truncated | REJECTED | 截断 PDF → 结构化错误,无 panic |
| pdf_garbage | REJECTED | 4KB 垃圾 → Invalid header |
| pdf_encrypted | HANDLED | 检出 `/Encrypt` → 一期按不支持处理,不解密 |
| pdf_valid_control | OK | **正对照**:合法单页解析成功(拒绝非全拒) |
| docx_zip_path_traversal | REJECTED | `../../evil.txt` → `enclosed_name()` 返回 None |
| **docx_zip_over_cap_rejected** | REJECTED | **真实对抗**:声明解压 2MB > CAP 1MB,解压前拒绝(fail-closed);**变异验证**:去掉 `declared > CAP` 判定 → all_safe=false |
| xml_entity_expansion | REJECTED | billion-laughs 样本检出 DOCTYPE/ENTITY → 拒(生产 XML 须禁 DTD) |
| **crash_containment** | CONTAINED | **子进程** worker 在解析中 `abort()`,父进程存活;含 5s 超时 kill 机制 |

相比 v1 的改进(全部回应 codex 复核):
- **#2**:zip 炸弹探针从"能读元数据"改为**真实超限拒绝 + 变异证据**;
- **#3**:崩溃遏制从同进程 `catch_unwind` 改为**独立可杀子进程** + 超时;
- **#5**:证据产物是**合法 JSON**(正确转义),控制台标记与 JSON 分离,
  内置 well-formed 校验 + python json.tool 外部验证。

## 仍未覆盖(NOT-RUN,归功能面 spike,gates W3)

- 数值资源档位实测:内存/CPU/墙钟上限的具体数字(本 spike 验机制存在,未压测);
- 真实样本抽取率:中文/表格/多栏 + 锚点回源逐字一致;
- 扫描件(无文本层→「无法定位」)、DOCX 段落/块级锚点与 run 拆分;
- 加密文档一期确定"不支持"的完整 UX(本 spike 只验解析层不崩)。

## 结论(收窄后)

W1 **安全面机制的受测向量全部 fail-closed**,依赖图为纯 Rust deflate。
这**不等于**安全门关闭——数值档位与功能面待补。pdfium→纯 Rust 的选型修订
是**建议**,待功能面 spike 通过后才落地。W2 骨架不依赖本修订即可开工。
