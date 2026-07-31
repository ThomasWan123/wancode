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
| 07-24 | 【用户实报+当日修复】排队条目 ⚡/✕/Clear 点了没反应。根因:ext_notify 注入 owner=wancode,入队条目 owner=None,引擎守卫永不匹配→静默 no-op（与 yolo_mode 同型坑第二次踩:单客户端不传身份标识）。↑↓ 是单条禁用态误会;✏ 有效因 edit 不校验 owner。用户关键追问「你自己(Claude Code)就是这样的么?先确认自己再更新」→纠正对齐方向:Claude Code 忙时消息=注入当前回合(非排队等待)。v0.18.4:默认翻转(Enter=interject/Alt+Enter=排队)+owner 修复+队列行 UI 简化 | 成（当日闭环） | —— | —— | 教训:对标自己用的工具时先实证其真实行为,别照截图想当然 | glm-5.2 | 高（已修复） | ext_parsers.rs owner 守卫;v0.18.4;commit 4073a18 |
| 07-25 | 【用户实报+当日修复】设置页手动新增模型:①Save 后聊天区下拉不出现,须重启（实锤:引擎启动读一次 config,upsert 只写盘）——v0.18.5 修复:Save 后调 x.ai/internal/reload_models 热重载+条目键即时并入下拉;②称 base_url 串到智谱非 coding 端点——干净复现失败（127.0.0.1 假端点模型实测路由正确,错误 URL 即所配端点）,唯一可疑机制=resolve_catalog_key 按 Model id 字段兜底扫描在条目重名时跨条目命中（会话恢复路径存 Model id 而非条目键）,待用户提供当时填写值后针对性加固 | 成（①闭环②待复现材料） | —— | —— | 侧栏会话 model 显示用 model 字段而非条目键（同一脆弱性的表征） | glm-5.2 | ①中②高（待证） | m3/m4.png;v0.18.5;commit 597d3cd |

## 已知摩擦（建设期自举中预先记录，dogfooding 中验证频率）

- 审查行号与工作区漂移（prompt 已声明"仅供参考"，看真实误导率）
- 中文 IME 下 @ 联想/命令面板直接键入走候选窗
- 预览 iframe 聚焦后全局快捷键失效（需点外部恢复）
- Review 偶发空产出一例（未复现，留意）
- RECENTS 临时会话快照残影（已加 refreshSessions，观察是否根除）
- 切换/新建会话后上一会话的 Plan 面板与 Terminal 面板残留不清理（Home 页被 Plan 挤占,composer 需滚动;07-24 v0.18.4 测试中实见）
- v0.18.4 实测记录:忙时 Enter 注入当前回合（模型精确回复 INTERJECT-OK-777）与 Alt+Enter 排队（回合结束后作为新回合执行）双路径 E2E 通过（t11.png）

## 2026-07-28 验证期收尾（授权 AI 全量验证）

| 项 | 结果 | 证据 |
|---|---|---|
| 发布态全套自动化（干净引擎+main） | ✅ 全绿 | 路由 3/3、身份链 2/2、lib 81、金丝雀 8、ACP 全流程 1、RTL 14、tsc/build 干净 |
| 引擎级 smoke（真实模型 API） | ⚠ 5/6 | S2 失败为智谱 Coding Plan 真实 429（7-30 重置），非产品 bug → #122 复测 |
| 自动更新链路（G19） | ❌ 发现缺陷 | 用户实证：下载后无界面不重启，注册表仍 0.18.5。镜像 sha256=官方一致；手动 /S 安装 exit=0 升级成功 → 根因指向"静默安装无反馈 + relaunch 永不执行"，#121 |
| 升级到 v0.18.6 | ✅ 已完成 | 以更新器同款方式安装，注册表 0.18.6 |

验证期结论：两周报 7 个真实问题（图片 400、链接覆盖、历史图片、队列按钮、
模型不显示+端点串台、恢复覆盖、更新无反馈），全部当天定位；前 6 个已修复
发版，第 7 个立项 v0.18.7。优先级最高的下一项 = #121 更新链路 UX。

## 2026-07-30 v0.18.7 发布

| 发布门项 | 结果 |
|---|---|
| #122 正式 S2（智谱 Coding Plan 原配置） | ✅ smoke 6/6，S2-reply EndTurn 落盘 |
| DeepSeek 通用链路预检（前一日） | ✅ 6/6 |
| release 构建 + 补签 + latest.json | ✅（release.ps1 在 powershell 5.1 下必死：EAP=Stop + 原生 stderr 包装，改 bash 直跑；脚本待加 5.1 检测） |
| GitHub release v0.18.7 四资产 | ✅ setup/msi/.sig/latest.json |
| latest.json 线上回读 | ✅ version=0.18.7、镜像 URL |
| 镜像字节 | ✅ 首字节 MZ，本地=官方 sha256 一致 |
| 0.18.6→0.18.7 隔离升级 E2E | ✅ 26/26 OVERALL PASS（含双版本 minisign 验签、/R 拉起、注册表逐字节恢复、真实安装未动） |

E2E 首跑暴露脚本断言 bug：真实安装版本硬编码 ==NewVersion（首轮 0.18.6 恰好
相等被巧合掩盖），已改为"与跑前快照一致"；期间脚本 fail-safe 兜底正确触发
（异常→安全中止→注册表恢复）。版本对已参数化（WUE_OLD/WUE_NEW）。

## 2026-07-31 #129 真机应用内升级验收（0.18.7-test → 0.18.8-rc.1）

| # | 验收项 | 结果 | 证据 |
|---|---|---|---|
| 1 | 应用内发现 + 下载 | PASS | prerelease latest-test.json 命中 0.18.8-rc.1；暂存 `%TEMP%\wancode-updater-0.18.8-rc.1-<rand>` 完整安装器 24732136B（tempfile 随机后缀生效） |
| 2 | 安装器从 Job 内 breakaway | PASS | 应用退出（旧 pid 111428）后安装器存活并完成安装——正是 v0.17.0 事故形状的反面 |
| 3 | 退出 + 安装 + 自动重启 | PASS | 全链 ~8s；/R 生效，新 pid 114228 自隔离目录拉起 |
| 4 | 版本/配置/会话对账 | PASS | exe ProductVersion=0.18.8-rc.1；About 显示 v0.18.8-rc.1；config.toml 在（sha B0034454…）；sessions 2032→2044 仅新增不丢 |

隔离与恢复：/D 隔离安装；注册表快照→导入恢复（DisplayVersion=0.18.7、InstallLocation=真实目录）；真实安装 sha 875F3504… 全程不变；隔离目录与暂存已清理。

定向补测（同日第二轮，~150ms 连拍 129 帧）：已捕获真实 UI 帧——
"Downloading v0.18.8-rc.1... 0% / 16%"（下载进度）与 "v0.18.8-rc.1
downloaded. The app will close and install automatically (a progress bar
will appear), then reopen itself."（装前提示）；随后帧序列显示应用退出、
重启回到首页。第二轮同样升级成功（exe=0.18.8-rc.1）、注册表恢复、
真实安装未动。四项验收全部具备直接 UI 证据。

结论：#129 修复在真机应用内验证通过。正式 0.18.8 待放行。

## 2026-07-31 v0.18.8 正式发布（#129 收官）

- 构建：main 88c0252，不含 updater-test-endpoint；防串线扫描主 exe 无 latest-test.json / v0.18.8-rc.1 / feature 名残留。
- 资产 4 项：setup 24735622B（sha256 45d2d44f…）、.sig、msi 46764032B（fc714400…）、latest.json（仅指向 v0.18.8，gh-proxy 镜像 URL）。
- 发布后验证：官方直链与镜像双下载 sha256 与本地一致；MZ 头正确；minisign（Ed25519+BLAKE2b，本地纯 python）双源 OK，keyid 01b6abd024deb275；.sig 与 latest.json signature 逐字一致。
- v0.18.8 notes 与 v0.18.7 页面均已写明"本次必须手动安装一次"；v0.18.7 四资产未动。
- 本机真实安装已手动升级 0.18.8（install exit=0，注册表 0.18.8，config.toml sha 前后一致 B0034454…）。
- 审计证据保留：rc/0.18.8-rc.1 分支 + v0.18.8-rc.1 prerelease。
- #129 关闭；#127 解冻排 v0.18.9；#126 待单独设计。
