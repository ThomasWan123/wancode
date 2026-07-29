# 模型兼容性定位与矩阵

> 定位（2026-07-29 定）：**底层是多模型、OpenAI 兼容；产品体验与验证深度
> GLM Coding Plan 优先。** 对外表述统一为：支持 OpenAI-compatible
> endpoints；GLM 与 DeepSeek 已验证，其他服务商按兼容程度接入。
> 不再使用"任意 OpenAI 兼容端点"的宽表述。

## 兼容矩阵（随发布更新）

| 模型/协议 | 等级 | 证据 |
|---|---|---|
| GLM Coding Plan | 完整验证 | 默认配置、向导、联网搜索、发布门真实 API 主路径、全部 dogfooding |
| 智谱开放平台 | 完整验证 | 与 Coding Plan 端点/Key 分离；同 slug 双端点路由为 v0.18.6 全套测试对象 |
| DeepSeek Chat/R1 | 核心验证 | 2026-07-29 真实 API smoke 6/6（启动/回复/落盘/排队/插话/恢复/Git） |
| Ollama / One-API 等 | 基础兼容 | 理论兼容、可手动配置，未逐家认证 |
| Anthropic / Gemini 原生 | 未支持 | 需 OpenAI 兼容代理或新增适配层；v0.19 按真实需求决策 |

## "OpenAI 兼容"的已知风险面

Tool Calling 字段格式 / SSE 结束标记 / reasoning 字段 / 多模态消息 /
usage 与错误/限流格式 / 上下文长度与压缩阈值 / system-developer role /
辅助模型路由 / 并行工具调用 / 认证方式。历史兼容补丁（object 字段宽容化、
4v-flash max_tokens、429 文案）均源于此类差异。

## v0.18.8 兼容性治理（范围定义）

1. 模型能力声明：Text / Tool Use / Vision / Reasoning / Image Description / Context Window
2. UI 能力徽章；不支持图片的模型在发送前直接提醒（呼应"垫底逻辑"常设要求）
3. Provider 合规测试套件：基本回复、流式、Tool Calling、错误解析、上下文压缩、多模态、凭据与端点隔离
4. 本矩阵随每次发布更新

v0.18.7 不扩适配范围，先稳定发布。
