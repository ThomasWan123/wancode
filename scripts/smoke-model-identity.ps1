# v0.18.6 模型身份真机冒烟夹具
#
# 把"自己配一套能触发歧义的环境"变成"跑一条命令，然后点七下"。
#
# 为什么需要真机这一步：RTL 与引擎集成测试各自覆盖了自己那一层，但没有
# 任何自动化测试跑过 MvpAgent::load_session → x.ai/modelBlock → Tauri →
# React 这条完整链路。历史上出问题最多的恰恰是层与层之间的接缝
# （endpoint_label 的 snake_case/camelCase 错配就是典型）。
#
# 隔离性：全部写进临时 GROK_HOME，绝不碰你真实的 ~/.grok 与真实 Key。
# 两个"模型"指向本地 mock 端点，不需要任何真实 API Key，也不会发出外网请求。

param(
    # 保留夹具目录以便反复试；默认用后清掉。
    [switch]$Keep,
    # 只造夹具、不启动 GUI。用于自检夹具本身是否成立。
    [switch]$SetupOnly
)

$ErrorActionPreference = 'Stop'

$root = Join-Path $env:TEMP "wancode-smoke-$(Get-Random)"
$grokHome = Join-Path $root ".grok"
New-Item -ItemType Directory -Force $grokHome | Out-Null

# ── 两个本地 mock 端点：同一个上游 slug，不同 host ────────────────────
# 用不同端口把它们区分开——真实事故里区分二者的正是端点。
$listeners = @()
function Start-MockEndpoint([int]$port, [string]$tag) {
    $job = Start-Job -ArgumentList $port, $tag, $root -ScriptBlock {
        param($port, $tag, $root)
        $log = Join-Path $root "$tag.requests.log"
        $http = [System.Net.HttpListener]::new()
        $http.Prefixes.Add("http://127.0.0.1:$port/")
        $http.Start()
        while ($http.IsListening) {
            $ctx = $http.GetContext()
            $auth = $ctx.Request.Headers['Authorization']
            Add-Content -Path $log -Value "$(Get-Date -Format o) $($ctx.Request.Url.AbsolutePath) auth=$auth"
            $body = @(
                'data: {"id":"smoke","object":"chat.completion.chunk","created":0,',
                '"model":"glm-4.6","choices":[{"index":0,"delta":{"role":"assistant",',
                '"content":"[' + $tag + ' 收到请求]"},"finish_reason":"stop"}]}',
                '', 'data: [DONE]', ''
            ) -join ''
            $bytes = [Text.Encoding]::UTF8.GetBytes($body)
            $ctx.Response.ContentType = 'text/event-stream'
            $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
            $ctx.Response.Close()
        }
    }
    return $job
}

$listeners += Start-MockEndpoint 34101 'OPEN'
$listeners += Start-MockEndpoint 34102 'CODING'
Start-Sleep -Milliseconds 600

# ── 配置：两个条目共享上游 slug glm-4.6，端点与 Key 都不同 ────────────
# 这正是用户报的那个事故的形状（开放平台 vs Coding Plan）。
$config = @'
[model.glm-open]
name = "模拟·开放平台"
model = "glm-4.6"
base_url = "http://127.0.0.1:34101/v1"
api_key = "key-for-open"
api_backend = "chat_completions"
context_window = 128000

[model.glm-coding]
name = "模拟·Coding Plan"
model = "glm-4.6"
base_url = "http://127.0.0.1:34102/v1"
api_key = "key-for-coding"
api_backend = "chat_completions"
context_window = 128000

[model.solo]
name = "模拟·无歧义模型"
model = "solo-slug"
base_url = "http://127.0.0.1:34101/v1"
api_key = "key-for-open"
api_backend = "chat_completions"
context_window = 128000
'@
[IO.File]::WriteAllText((Join-Path $grokHome 'config.toml'), $config, [Text.UTF8Encoding]::new($false))

# ── 一份旧格式会话记录：只有 slug，没有 catalog_model_id ──────────────
# 这就是 v0.18.6 之前所有会话的形状，也是歧义的来源。
$workspace = Join-Path $root "proj"
New-Item -ItemType Directory -Force $workspace | Out-Null
Set-Content (Join-Path $workspace "README.md") "smoke fixture"

$sessionId = "smoke-legacy-session"
$sessionDir = Join-Path $grokHome "sessions\$sessionId"
New-Item -ItemType Directory -Force $sessionDir | Out-Null
$summary = @{
    info               = @{ id = $sessionId; cwd = $workspace }
    session_summary    = "冒烟夹具：旧格式会话"
    created_at         = (Get-Date).ToUniversalTime().ToString('o')
    updated_at         = (Get-Date).ToUniversalTime().ToString('o')
    num_messages       = 0
    num_chat_messages  = 0
    # 关键：只有 slug，没有 catalog_model_id。
    current_model_id   = "glm-4.6"
    next_trace_turn    = 0
    chat_format_version = 1
} | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText((Join-Path $sessionDir 'summary.json'), $summary, [Text.UTF8Encoding]::new($false))
New-Item -ItemType File -Force (Join-Path $sessionDir 'chat_history.jsonl') | Out-Null

Write-Host ""
Write-Host "夹具就绪" -ForegroundColor Green
Write-Host "  GROK_HOME : $grokHome"
Write-Host "  工作区    : $workspace"
Write-Host "  旧会话    : $sessionId （只有 slug glm-4.6，无 catalog key）"
Write-Host "  端点日志  : $root\OPEN.requests.log / $root\CODING.requests.log"
Write-Host ""
Write-Host "请核对这七项：" -ForegroundColor Cyan
Write-Host "  1. 恢复上面那个旧会话 → 弹出选择器，列出两个候选，各自显示"
Write-Host "     真实端点（127.0.0.1:34101 与 :34102）。端点为空即为失败。"
Write-Host "  2. 此时发送按钮不可用。"
Write-Host "  3. 点『稍后再说』→ 弹窗收起，提示条仍在，发送仍不可用；"
Write-Host "     点提示条可重新展开。"
Write-Host "  4. 选『模拟·Coding Plan』→ 发一条消息 → 只有 CODING.requests.log"
Write-Host "     增加记录，OPEN.requests.log 不变。"
Write-Host "  5. 切走再恢复同一会话 → 不再询问（身份已写回）。"
Write-Host "  6. 切到一个新建的普通会话 → 弹窗与提示条完全消失。"
Write-Host "  7. 编辑 config.toml 删掉 glm-coding 段，重启后恢复该会话 →"
Write-Host "     出现『模型已不在配置中』提示，从下拉另选一个模型可解除。"
Write-Host ""

if ($SetupOnly) {
    $listeners | ForEach-Object { Stop-Job $_ -EA SilentlyContinue; Remove-Job $_ -Force -EA SilentlyContinue }
    Write-Host "SetupOnly：夹具已构造，未启动 GUI。目录 $root" -ForegroundColor Yellow
    return
}

$env:GROK_HOME = $grokHome
Write-Host "正在以隔离配置启动 WanCode（关闭窗口即结束）…" -ForegroundColor Yellow
try {
    npm run tauri dev
}
finally {
    $listeners | ForEach-Object { Stop-Job $_ -ErrorAction SilentlyContinue; Remove-Job $_ -Force -ErrorAction SilentlyContinue }
    Write-Host ""
    Write-Host "端点命中统计：" -ForegroundColor Cyan
    foreach ($tag in 'OPEN', 'CODING') {
        $log = Join-Path $root "$tag.requests.log"
        $n = if (Test-Path $log) { (Get-Content $log | Measure-Object -Line).Lines } else { 0 }
        Write-Host "  $tag : $n 次请求"
    }
    if ($Keep) {
        Write-Host "夹具保留在 $root" -ForegroundColor Yellow
    } else {
        Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
        Write-Host "夹具已清理（加 -Keep 可保留）"
    }
}
