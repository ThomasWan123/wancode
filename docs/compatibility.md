# 模型兼容性定位与矩阵

> 定位（2026-07-29 定）：**底层是多模型、OpenAI 兼容；产品体验与验证深度
> GLM Coding Plan 优先。** 对外表述统一为：支持 OpenAI-compatible
> endpoints；GLM 与 DeepSeek 已验证，其他服务商按兼容程度接入。
> 不再使用"任意 OpenAI 兼容端点"的宽表述。

## 兼容矩阵（随发布更新）

| 模型/协议 | 等级 | 证据 |
|---|---|---|
| GLM Coding Plan | 完整验证 | 默认配置、向导、联网搜索、发布门真实 API 主路径、全部 dogfooding。另有 **Mock 协议合规**证据见下表（不改变真实服务验证等级） |
| 智谱开放平台 | 完整验证 | 与 Coding Plan 端点/Key 分离；同 slug 双端点路由为 v0.18.6 全套测试对象 |
| DeepSeek Chat/R1 | 核心验证 | 2026-07-29 真实 API smoke 6/6（启动/回复/落盘/排队/插话/恢复/Git）。R1 `reasoning_content` 形状另有 **Mock 协议合规**证据（A4 情景，不改变真实服务验证等级） |
| Ollama / One-API 等 | 基础兼容 | 理论兼容、可手动配置，未逐家认证 |
| Anthropic / Gemini 原生 | 未支持 | 需 OpenAI 兼容代理或新增适配层；v0.19 按真实需求决策 |

## "OpenAI 兼容"的已知风险面

Tool Calling 字段格式 / SSE 结束标记 / reasoning 字段 / 多模态消息 /
usage 与错误/限流格式 / 上下文长度与压缩阈值 / system-developer role /
辅助模型路由 / 并行工具调用 / 认证方式。历史兼容补丁（object 字段宽容化、
4v-flash max_tokens、429 文案）均源于此类差异。

## 合规套件证据（v0.18.9，#127-4）

> 证据纪律：每条含测试落点、CI run、日期与 commit——不写裸"已验证"。
> 证据类型三分：**Mock 协议合规**（本地 mock × 生产 ACP 链）/ **真实 API smoke** / **引擎层单测**——互不替代，Mock 合规不单独提升真实服务的兼容等级。
> 权威 run：[30698185882](https://github.com/ThomasWan123/wancode/actions/runs/30698185882)（main push）。tested main SHA = `f604792ffbd6c8a6190208b4670cd980ca2d2ae3`；artifact 内 `ci_sha` 与之**一致**。日期：2026-08-01。
> artifact 90 天过期——两份摘要内容与其 SHA-256 已固化至 [`docs/evidence/provider-compliance-v0.18.9.json`](evidence/provider-compliance-v0.18.9.json)，长期可审计。

| 维度 | 证据类型 | 证据形态 | 落点 |
|---|---|---|---|
| 传输/流式：标准流（带 usage 终块）、无 `[DONE]` 哨兵、无 usage 字段、`reasoning_content`（正向进 `agent_thought_chunk` + 反向不混正文） | Mock 协议合规 | 生产 ACP 链路 × 情景 mock，7 情景 | [`provider_compliance.rs`](../src-tauri/tests/provider_compliance.rs) · artifact `compliance-summary-4a` |
| 错误解析：401（带状态+供应商 message）、429（归类限流）、500+非 JSON 体（含状态、`max_retries=1` 恰好 2 次请求实证重试） | Mock 协议合规 | 同上，逐情景区分断言、错误绝不含 Key | 同上 |
| 工具调用：单调用往返（`tools` 结构化声明 + `tool_call_id` 精确对应）；**并行调用协议/批量工具调用支持**（一个 delta 两条调用、结果同请求齐备——不宣称测量执行时间重叠） | Mock 协议合规 | 生产 ACP 链路，请求体形状断言 | [`provider_compliance_4b.rs`](../src-tauri/tests/provider_compliance_4b.rs) · artifact `compliance-summary-4b` |
| 多模态路由：转述开启"图片 → helper → 描述标记进主模型"整链闭环；转述关闭图片内联、helper 零调用 | Mock 协议合规 | 同上（双 mock，描述标记 `VISION-DESCRIPTION-4B` 透传断言） | 同上 |
| 凭据与端点隔离 | 引擎层集成测试 | **引用引擎 Gate 1 测试**（正确端点 1 次、错误端点 0 次、Authorization 归属） | 引擎 `tests/model_endpoint_routing.rs` · CI 步"Gate 1 引擎侧路由证据" |
| 上下文压缩 | 引擎层单测 | **引擎层覆盖**（非 ACP 级重演，如实区分于"通过"） | 引擎 crate `xai-grok-compaction` 128 条单测 · CI 步"引擎压缩单测" |

### 套件挖出的兼容性知识（已入产品/测试）

- **引擎输入约束**（非 Provider 限制）：发送前有两道图片尺寸门——边长 ≥8 且总像素 ≥512（`image_dropped_notice`），过小图片被丢弃并降级为文字说明。
- **配置契约**：`[models].image_description` 优先按**slug 唯一解析**；歧义阻断（fail-closed）；仅在零 slug 匹配时才兼容 catalog key 写法；均无则不可路由（PR #11 产品修复）。测试：`helper_slug_resolves_when_key_differs` / `duplicate_helper_slug_fails_closed` / `literal_key_must_not_wash_out_slug_ambiguity` / `exact_key_fallback_when_no_slug_matches`。
- 5xx 由引擎按 `max_retries`（默认 15，指数退避）重试——限时验证需在配置收敛预算。

## v0.18.9 兼容性治理（范围定义）

1. 模型能力声明：Text / Tool Use / Vision / Reasoning / Image Description / Context Window
2. UI 能力徽章；不支持图片的模型在发送前直接提醒（呼应"垫底逻辑"常设要求）
3. Provider 合规测试套件：基本回复、流式、Tool Calling、错误解析、上下文压缩、多模态、凭据与端点隔离
4. 本矩阵随每次发布更新

v0.18.8 为更新器 P0 热修专版；治理自 v0.18.9 启动。
