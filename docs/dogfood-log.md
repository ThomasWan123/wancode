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
| 07-23 | 【复盘修正+修复】上一条误诊：真因是引擎 image_describe 转述管线被 `is_cursor_harness()`（硬编码 false）关死，图片一直走内联。v0.18.1 四连修：①转述管线经 `GROK_IMAGE_TRANSCRIBE=1` 启用（WanCode 默认开）②describe max_tokens 4096→env 可调（4v-flash 上限 1024）③转述后不再把原图挂进对话项④describe 失败垫底降级（图片存盘+路径引用+降级说明，绝不报错中断——产品决策）。E2E 双路径实证：glm-5.2 主模型粘图，思考块自述"I'm given a description of the image"并答对（e2e2.png）；垫底路径降级说明注入、回合继续（fb1.png） | 成 | —— | —— | 遗留观察：垫底后模型可能主动 read_file 读图再撞 400（文案已加禁止指示，工具层能力门控留 v0.19）；旧会话历史内联图片在切纯文本模型后仍可能毒害后续回合 | glm-5.2+glm-4v-flash / 转述约 8s | 低（已修复） | e2e2.png/fb1.png；vendor patch 四处 |
| 07-23 | 【用户实报+当日修复】开发网站点 demo 地址：WebView 整页导航，对话界面被目标网页覆盖（无服务端口则似无反应）。根因：ReactMarkdown 链接无拦截，Tauri WebView `<a href>` 默认当前页导航。v0.18.2 修复：App 级全局捕获 http(s) 链接点击→openUrl 系统浏览器。E2E：点链接后 App 界面完好，Chrome 正常接管 | 成（当日闭环） | —— | —— | 排障插曲：打开的 Chrome 错误页窗口盖住 WanCode，SetForegroundWindow 被拒，连续误判截图内容——按 pid+rect 枚举窗口才定位；发版教训：taskkill /IM 会误杀用户安装版实例（应只杀 dev exe）；git add -A 混入用户 demo 产物 blog-demo（已移出+gitignore） | —— | 高（已修复） | link2.png；v0.18.2；commit 693b670 |
| 07-24 | 【用户实报+当日修复】0.11 时代旧会话（含 read_file 读图历史）升级 0.18.2 后续聊仍 400——历史 ToolResult.images 随上下文发给纯文本端点。v0.18.3：主回合请求前统一消毒（User Image 块→占位文本;ToolResult.images 清空+附说明）,纯函数+金丝雀单测,CI 绿关账。排障链:误诊三层（安装版实为 0.11→升级;新粘图已好→旧会话仍炸;首版消毒漏 ToolResult） | 成（当日闭环） | —— | —— | 环境障碍:SentinelOne 锁 debug dll 本地单测三连败（CI 兜底）;git credential-manager 后台挂死（gh token 直推解） | glm-5.2 | 高（已修复） | unified.jsonl;v0.18.3;commit 1ff2745 |

## 已知摩擦（建设期自举中预先记录，dogfooding 中验证频率）

- 审查行号与工作区漂移（prompt 已声明"仅供参考"，看真实误导率）
- 中文 IME 下 @ 联想/命令面板直接键入走候选窗
- 预览 iframe 聚焦后全局快捷键失效（需点外部恢复）
- Review 偶发空产出一例（未复现，留意）
- RECENTS 临时会话快照残影（已加 refreshSessions，观察是否根除）
