# C1 Cowork 逃逸探针 spike — 证据报告

> 对照 `docs/design/v0.20-work-cowork-increment.md` §2.1 逃逸探针。
> spike 代码:`spike/c1-escape/`(独立 workspace)。证据:`c1-escape-probe.json`。

## 档位裁定:**档 B**(证据支持,与设计预期一致)

三个逃逸向量在真实文件系统上实测,加一个 worktree 内合法写正对照:

| 向量 | 谓词判定 | 说明 |
|---|---|---|
| in_worktree_control(正对照) | ALLOW | worktree 内路径判定在内 —— 谓词不是"全拒" |
| absolute_path | WOULD_BLOCK | 绝对路径写宿主,谓词识别为越界 |
| dotdot_relative | WOULD_BLOCK | `subdir/../../host/x` 逃逸,canonicalize 解析后识别越界 |
| symlink_junction | WOULD_BLOCK | 经 worktree 内 junction 写宿主(本机 symlink 需权限,改用 junction),canonicalize 解析链接后识别越界 |

`sentinel_intact: true`(宿主哨兵字节/存在性不变);`predicate_catches_all_tested: true`。

## 决定性结论:谓词能识别 ≠ 有地方拦截

**这是本探针最重要的一条**:`canonicalize + starts_with` 谓词(客户端能做到
的最强路径限制)**能识别全部三个逃逸向量**——但档位仍是 **B**,因为:

- `enforcement_point_exists: false` —— PR #39 已确证:引擎在放行 shell 写
  之前**没有调用点**会咨询这个谓词。工具/MCP 策略只在 `NewSessionRequest`
  构造期设定(`agent.rs:453-482`),`PromptRequest` 无逐操作钩子。
- 谓词识别逃逸 ≠ 在写发生前拦下它。档 A 需要**写前强制拦截执行点**,
  在 G26 零引擎改动下不存在。

因此 **Cowork 一期按档 B 落地**:每个后台任务启动前弹显式确认框(同屏示
worktree 与宿主路径),未确认不启动(G25 反向断言)。这不是残缺——是对
"worktree 只隔离 Git、不隔离文件系统"这一事实的诚实呈现。

## 档 A 的未来升级触发条件(精确化)

档 A 仅当出现**写前强制拦截**机制时可重启探针,且该机制须在写发生前被
调用。候选(均需受审计的引擎接口或 OS 机制,非本版范围):
- 引擎侧逐操作文件写钩子 + 客户端谓词(需 G26 例外 PR);
- OS 级文件系统强制(Windows 无进程级原语,见 security-sandbox-assessment)。
- **ETW 不构成升级路径**(事后检测,观察≠阻止,已在设计 §2.1 R3-F3 钉死)。

## 诚实边界(NOT-RUN)

- 本 spike 测的是**谓词能力**(能不能识别逃逸)+ 执行点缺失这一已知事实,
  **未**驱动完整 MvpAgent 会话让引擎真发 shell tool_call。后者是 C1 第二
  阶段(provider_compliance 级夹具),用于产出"真实 tool_call 记录 +
  结构化拒绝"——但其结论不会改变档位(执行点缺失是结构性的)。
- TOCTOU:谓词判定与实际写之间的竞态未评估(档 B 下由确认门 + 合并前
  人工验收兜底,不依赖谓词强制)。

## 结论

Cowork = **档 B**,证据充分。设计 §1.1 的预期("Windows 无进程级文件沙箱
原语")得到实证支持,且给出了比预期更精确的机制级解释(谓词够强、执行点
缺失)。C2 单任务闭环可按档 B 开工,确认门是硬性前置。
