# Dogfooding 日志（使用验证期：2026-07-22 起 10–14 天）

> 规则：WanCode 作为主力工具做真实项目。每个任务记五件事，两周后按
> 真实失败频率定下一版方向。不预排功能，不为对标率接 RPC。
> 目标覆盖：一个前端项目（预览/Diff/审查/修复）、一个 Rust/后端项目
> （终端/测试/PR/CI）、一个并行任务项目（子 Agent/后台/worktree）。

## 记录模板（复制一行）

| 日期 | 项目/任务 | 结果(成/部分/败) | 人工救场点 | 错误恢复(重启/改配置/无) | 操作摩擦 | 模型/耗时/重试 | 严重度(阻断/高/中/低) | 证据(日志/截图/commit/PR) |
|---|---|---|---|---|---|---|---|---|
| 07-23 | 粘图问文（GLM-5.2） | 败（预期内） | 无 | 无需（会话可继续） | 图片粘贴/预览正常，但纯文本模型收图直接回原始 API 400 JSON（content.type 参数非法，取值范围 ['text']），小白无法理解失败原因；应发送前按模型能力拦截或提示切视觉模型 | glm-5.2 / 秒回 400 / 0 重试 | 中 | 截图 img3.png；coding 端点 400 报文 |
| 07-23 | 粘图问文（glm-4v-flash，引擎 object 补丁后） | 成 | 引擎补丁+加模型（开发者级操作，小白无法自助） | 无 | ①智谱 4V 响应缺 OpenAI `object` 字段，引擎 serde 必填炸 `missing field`——已打 `#[serde(default)]` 宽容补丁（vendor patch 更新）；②引擎原生 image_describe 管线确认：图片永不内联发主模型，由 image_description 辅助模型转述——正确形态是任意主模型+4v 辅助 | glm-4v-flash / ~15s / 0 重试；回复精确 "WANCODE IMG TEST 777" | 中 | 截图 v6.png；types.rs 补丁 |
| 07-23 | 粘图问文（GLM-5.2 主 + image_description=glm-4v-flash） | 败 | 切主模型绕过 | 无需 | `[models].image_description` 指向 glm-4v-flash 后 describe 仍打主模型 coding 端点（resolve_aux 疑似凭证解析回退，与 disable_api_key_auth/BYOK env-key 交互待查）——辅助模型路由对 BYOK 模型不生效是"多模型分工"卖点的直接障碍 | glm-5.2 / 秒回 400 | 高 | 截图 v5.png；resolve_aux_model_sampling_config 代码路径 |

## 已知摩擦（建设期自举中预先记录，dogfooding 中验证频率）

- 审查行号与工作区漂移（prompt 已声明"仅供参考"，看真实误导率）
- 中文 IME 下 @ 联想/命令面板直接键入走候选窗
- 预览 iframe 聚焦后全局快捷键失效（需点外部恢复）
- Review 偶发空产出一例（未复现，留意）
- RECENTS 临时会话快照残影（已加 refreshSessions，观察是否根除）
