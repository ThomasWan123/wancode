# W1 解析 spike — 探索性 API 可行性(v3,**非**安全门关闭证据)

> 对照 `docs/design/v0.20-work-cowork-increment.md` §1.1。spike:`spike/w1-parser/`。
> 证据:`w1-parser-spike.json`(serde_json 产出 + parse-back 校验)。
>
> **范围(codex R2-F1)**:本 spike 是**探索性 API 可行性**,**不**关闭/计入
> W1 安全门。数值资源档位、真实样本抽取率、完整功能面均 NOT-RUN(归 W3 前置
> 的功能面 spike)。压缩栈声明精确到:纯 Rust deflate(miniz_oxide),无单独
> 分发的原生二进制;**不**宣称整依赖图零原生。

## 探针 10/10(all_safe=true;serde_json parse-back 通过)

| 探针 | 结局 | 真实性说明 |
|---|---|---|
| pdf_truncated | REJECTED | 真喂 lopdf,结构化错误 |
| pdf_garbage | REJECTED | 真喂 lopdf |
| pdf_valid_control | OK | 正对照,合法单页解析成功 |
| **pdf_encrypted_detected** | HANDLED | **构造结构合法 + trailer 带 /Encrypt 的 PDF**,解析后真检出 `/Encrypt` → 分类不支持(codex R2-F2:不再是伪装的截断测试) |
| docx_zip_path_traversal | REJECTED | `enclosed_name()` 对 `../` 返回 None |
| **docx_zip_over_cap_rejected** | REJECTED | 声明解压 2MB > CAP 1MB,解压前拒;**变异**:去判定 → all_safe=false |
| **docx_xml_entity_real_parse** | REJECTED | **真喂 `docx_rs::read_docx`**(billion-laughs DOCX),3s 内有界返回=**未无界展开挂死**(codex R2-F3:真实解析边界,非字符串检查) |
| docx_benign_control | NOT-RUN | 诚实标注:docx-rs 需真实世界完整 .docx,最小合成件不足以让它读 zip;良性正对照**留待 W3 真实样本**。因此实体探针仅证明"不挂死",未证明"结构正常解析" |
| **crash_containment** | CONTAINED | **独立子进程**解析中 `abort()`,非成功终止(0xC0000409),父存活 + 残留哨兵被清 |
| **hang_timeout_kill** | KILLED | **独立子进程**死循环,**超时被 kill**(触发超时分支),父存活 + 残留哨兵被清(codex R2-F4:crash/hang 分离 + 残留断言) |

## 相比 v2 的改进(全部回应 codex R2)

- **F1**:PR 标题/正文/注释收窄到"探索性、非门关闭";移除 CAP=200MB、
  catch_unwind、"零原生依赖"等失准表述;
- **F2**:加密探针用真实 /Encrypt PDF,真检出(非通用解析错误冒充);
- **F3**:XML 实体炸弹真喂 docx-rs,断言有界返回;良性对照如实 NOT-RUN;
- **F4**:crash 与 hang 分离;hang 触发超时 kill;两者断言残留哨兵清理;
- **F5**:serde_json 序列化 + 真实 parse-back(替代同义反复的括号配平)。

## 诚实边界(NOT-RUN)

- 数值资源档位(内存/CPU/墙钟上限具体值)——验机制存在,未压测;
- 真实样本抽取率、DOCX 段落锚点/run 拆分——归功能面 spike;
- **良性 DOCX 正对照**——需真实世界 .docx,留 W3。因此实体探针的结论限于
  "恶意 DOCX 不使 read_docx 挂死",不含"良性 DOCX 正常解析"。

## 结论(精确)

W1 **API 可行性成立**:纯 Rust 压缩栈、崩溃/挂死可遏制、结构对抗输入
fail-closed。这**不等于**安全门关闭——数值档位与功能面(含良性 DOCX 对照)
待补。pdfium→纯 Rust 选型是**建议**,待功能面 spike 后落地。W2 骨架可先行。
