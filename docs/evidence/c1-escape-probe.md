# C1 逃逸探针 — 谓词单元 spike(v2,已按 codex 复核收窄)

> **范围收窄(codex R1-F1/F3)**:本 spike **只测客户端路径谓词能否识别逃逸**,
> **不是** C1 完整证据——它不驱动真实引擎会话、无真实 tool_call、不观测结构化
> 拒绝,因此**不产出档位裁定**。设计 §2.1 的完整 C1 门要求 full-MvpAgent 阶段
> (真实 shell 写 + tool_call 记录 + 结构化拒绝 + 哨兵断言)。
>
> spike:`spike/c1-escape/`。证据:`c1-escape-probe.json`(合法 JSON)。

## 本 spike 证明了什么(谓词能力面)

真实文件系统上,`canonicalize + starts_with` 谓词对三个逃逸向量:

| 向量 | 谓词 | 说明 |
|---|---|---|
| in_worktree_control(正对照) | ALLOW | worktree 内路径判定在内(非全拒) |
| absolute_path | WOULD_BLOCK | 绝对路径写宿主被识别越界 |
| dotdot_relative | WOULD_BLOCK | `../../` 逃逸被识别越界 |
| symlink_junction | WOULD_BLOCK | 经 junction 写宿主,canonicalize 解链接后识别越界 |

退出码严格化(F2):三向量全部 WOULD_BLOCK + 正对照 ALLOW + 哨兵完好才 exit 0;
任一 ESCAPED/ERROR/SKIPPED → exit 1。

## 本 spike **没有**证明什么(诚实边界,codex R1-F1/F3 纠正)

- **不产出档位裁定**。上一版 hard-code `tier=B` / `enforcement_point_exists=false`
  是**过度宣称**——我把 PR #39 的 Work **会话级联网能力**边界错误外推到
  Cowork 的 **shell 写路径**。这是两条不同代码路径:PR #39 证明的是前者,
  **未**证明 Cowork 执行路径上没有可拦截的写策略/结构化拒绝点。
- 谓词"能识别"逃逸 ≠ 引擎在放行 shell 写前"会调用"它。是否存在可拦截点,
  必须由 full-MvpAgent 阶段实测,不能靠推断。

## C1 门:保留(用户裁定 2026-08-12)

用户裁定**保留 C1 完整门**:Cowork 档位裁定与 C2 开工要求 full-MvpAgent 实证
(真实引擎会话 + 真实 shell 写 tool_call + 观测结构化拒绝 + 哨兵断言)。本谓词
spike **不满足门、不产档位裁定**;它是必要非充分的输入(谓词能识别逃逸),
真正的档位由后续 full-MvpAgent 探针确立。
