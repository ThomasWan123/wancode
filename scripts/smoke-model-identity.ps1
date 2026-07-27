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
    # 只造夹具、不启动 GUI。用于自检夹具本身是否成立。
    [switch]$SetupOnly,
    # 第 7 项专用：从已有夹具里删掉两个 glm-4.6 条目，然后重新启动。
    # 不重造夹具，所以第 4 项写回的身份还在——这正是第 7 项要的前提。
    [switch]$Step7,
    # 留着上次的夹具继续用（默认每次重造，保证从旧格式记录开始）。
    [switch]$Resume
)

$ErrorActionPreference = 'Stop'

# 固定路径，不用随机临时目录——每次都变的话，第 7 项要你去翻新路径改 TOML，
# 中途也没法回头看端点日志。这个目录已进 .gitignore。
$root = Join-Path (Split-Path $PSScriptRoot -Parent) ".smoke-model-identity"
$grokHome = Join-Path $root ".grok"
$workspace = Join-Path $root "proj"
$configPath = Join-Path $grokHome 'config.toml'

if ($Step7) {
    if (-not (Test-Path $configPath)) { throw "还没有夹具，先不带 -Step7 跑一遍" }
    $cfg = Get-Content $configPath -Raw
    # 两个同 slug 条目必须都删：只删 glm-coding 时，glm-open 会成为唯一
    # slug 匹配并按兼容规则安全迁移，那不是 model_unavailable。
    foreach ($key in 'glm-coding', 'glm-open') {
        $cfg = [Regex]::Replace(
            $cfg,
            "(?ms)^\[model\.$([Regex]::Escape($key))\].*?(?=^\[|\z)",
            ''
        )
    }
    [IO.File]::WriteAllText($configPath, $cfg, [Text.UTF8Encoding]::new($false))
    Write-Host "已从配置中删除全部 glm-4.6 条目。" -ForegroundColor Green
    Write-Host "现在恢复那个会话，应当出现『模型已不在配置中』，且能从下拉另选一个解除。" -ForegroundColor Cyan
}
elseif (-not $Resume) {
    if (Test-Path $root) { Remove-Item -Recurse -Force $root }
}
New-Item -ItemType Directory -Force $grokHome | Out-Null

# ── 两个本地 mock 端点：同一个上游 slug，不同 host ────────────────────
# 用不同端口把它们区分开——真实事故里区分二者的正是端点。
$listeners = @()
function Start-MockEndpoint([int]$port, [string]$tag) {
    $job = Start-Job -ArgumentList $port, $tag, $root -ScriptBlock {
        param($port, $tag, $root)
        $log = Join-Path $root "$tag.requests.log"
        $serverLog = Join-Path $root "$tag.server.log"
        try {
            # HttpListener 在部分 Windows 环境需要 URL ACL，后台 Job 会直接
            # 失败而主脚本继续运行，造成“两个端点 0 次”的假验证。TcpListener
            # 只绑定 loopback，不需要管理员权限。
            $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $port)
            $listener.Start()
            while ($true) {
                # 不永久阻塞在 AcceptTcpClient：Stop-Job 只有在脚本有机会响应
                # 取消时才能及时退出，否则 SetupOnly/关闭 GUI 会挂在清理阶段。
                if (-not $listener.Pending()) {
                    Start-Sleep -Milliseconds 50
                    continue
                }
                $client = $listener.AcceptTcpClient()
                try {
                    $stream = $client.GetStream()
                    $reader = [IO.StreamReader]::new(
                        $stream, [Text.Encoding]::UTF8, $false, 4096, $true
                    )
                    $requestLine = $reader.ReadLine()
                    # 就绪探针只连端口、不发 HTTP；不计为模型请求。
                    if ([string]::IsNullOrWhiteSpace($requestLine)) { continue }
                    $auth = ''
                    while ($true) {
                        $line = $reader.ReadLine()
                        if ([string]::IsNullOrEmpty($line)) { break }
                        if ($line -match '(?i)^Authorization:\s*(.*)$') { $auth = $Matches[1] }
                    }
                    $path = ($requestLine -split ' ')[1]
                    Add-Content -Path $log -Value "$(Get-Date -Format o) $path auth=$auth"
                    $body = @(
                        'data: {"id":"smoke","object":"chat.completion.chunk","created":0,',
                        '"model":"glm-4.6","choices":[{"index":0,"delta":{"role":"assistant",',
                        '"content":"[' + $tag + ' 收到请求]"},"finish_reason":"stop"}]}',
                        "`n`ndata: [DONE]`n`n"
                    ) -join ''
                    $bodyBytes = [Text.Encoding]::UTF8.GetBytes($body)
                    $head = "HTTP/1.1 200 OK`r`nContent-Type: text/event-stream`r`n" +
                        "Content-Length: $($bodyBytes.Length)`r`nConnection: close`r`n`r`n"
                    $headBytes = [Text.Encoding]::ASCII.GetBytes($head)
                    $stream.Write($headBytes, 0, $headBytes.Length)
                    $stream.Write($bodyBytes, 0, $bodyBytes.Length)
                    $stream.Flush()
                }
                finally {
                    $client.Close()
                }
            }
        }
        catch {
            $_ | Out-String | Add-Content -Path $serverLog
            throw
        }
    }
    return $job
}

$listeners += Start-MockEndpoint 35101 'OPEN'
$listeners += Start-MockEndpoint 35102 'CODING'

# 启动门：两个端点未同时监听就立即失败，绝不让 GUI 在坏夹具上“通过”。
$ready = $false
for ($attempt = 0; $attempt -lt 20; $attempt++) {
    $ok = $true
    foreach ($port in 35101, 35102) {
        $probe = [Net.Sockets.TcpClient]::new()
        try {
            $probe.Connect('127.0.0.1', $port)
        }
        catch {
            $ok = $false
        }
        finally {
            $probe.Close()
        }
    }
    if ($ok) { $ready = $true; break }
    Start-Sleep -Milliseconds 100
}
if (-not $ready) {
    $failures = $listeners | Receive-Job -Keep -ErrorAction SilentlyContinue | Out-String
    $listeners | ForEach-Object { Stop-Job $_ -EA SilentlyContinue; Remove-Job $_ -Force -EA SilentlyContinue }
    throw "mock 端点未就绪，冒烟中止。$failures"
}

# ── 配置：两个条目共享上游 slug glm-4.6，端点与 Key 都不同 ────────────
# 这正是用户报的那个事故的形状（开放平台 vs Coding Plan）。
$config = @'
[model.glm-open]
name = "模拟·开放平台"
model = "glm-4.6"
base_url = "http://127.0.0.1:35101/v1"
api_key = "key-for-open"
api_backend = "chat_completions"
context_window = 128000

[model.glm-coding]
name = "模拟·Coding Plan"
model = "glm-4.6"
base_url = "http://127.0.0.1:35102/v1"
api_key = "key-for-coding"
api_backend = "chat_completions"
context_window = 128000

[model.grok-build-solo]
name = "模拟·无歧义模型"
model = "solo-slug"
base_url = "http://127.0.0.1:35101/v1"
api_key = "key-for-open"
api_backend = "chat_completions"
context_window = 128000
'@
if (-not $Step7 -and -not $Resume) {
    [IO.File]::WriteAllText($configPath, $config, [Text.UTF8Encoding]::new($false))
}

# ── 一份旧格式会话记录：只有 slug，没有 catalog_model_id ──────────────
# 这就是 v0.18.6 之前所有会话的形状，也是歧义的来源。
New-Item -ItemType Directory -Force $workspace | Out-Null
Set-Content (Join-Path $workspace "README.md") "smoke fixture"

# 固定 UUID 让隔离夹具更接近桌面端真实产生的会话，也便于重复运行时准确
# 定位同一条记录。此前“列表必须是 UUID”的猜测已被 ACP/列表集成测试证伪；
# 旧夹具消失的真正原因是工作区刷新竞态，不再保留那条错误诊断。
$sessionId = "00000000-0000-7000-8000-000000000186"
if (-not $Step7 -and -not $Resume) {
# 会话目录是 sessions/<百分号编码的 cwd>/<id>，不是 sessions/<id>。
# 摆错位置的话应用根本列不出这个会话——第一次跑就是这么空跑掉的：
# 应用开了默认工作区、新建了会话，夹具从头到尾没被碰过。
$cwdKey = [Uri]::EscapeDataString($workspace)
$sessionDir = Join-Path $grokHome "sessions\$cwdKey\$sessionId"
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
}

Write-Host ""
Write-Host "夹具就绪" -ForegroundColor Green
Write-Host "  GROK_HOME : $grokHome"
Write-Host "  工作区    : $workspace"
Write-Host "  旧会话    : $sessionId （只有 slug glm-4.6，无 catalog key）"
Write-Host "  端点日志  : $root\OPEN.requests.log / $root\CODING.requests.log"
Write-Host ""
# 应用里要"打开文件夹"时直接 Ctrl+V，不用手抄路径。
Set-Clipboard -Value $workspace
Write-Host "工作区路径已复制到剪贴板——应用里『打开工作区』时直接粘贴。" -ForegroundColor Green
Write-Host "第 7 项不用手改 TOML，另开一个终端跑：" -ForegroundColor Green
Write-Host "  powershell -ExecutionPolicy Bypass -Command `"cd $((Split-Path $PSScriptRoot -Parent)); .\scripts\smoke-model-identity.ps1 -Step7`"" -ForegroundColor DarkGray
Write-Host ""
# 冒烟必须跑分支代码，不能跑已安装的旧版。第一次跑就疑似跑成了 v0.18.5：
# 新建会话落盘为 current=glm-open（配置键）而不是 current=glm-4.6 + catalog=glm-open，
# 那是本分支之前的字段语义。
$branch = (git -C (Split-Path $PSScriptRoot -Parent) rev-parse --abbrev-ref HEAD 2>$null)
Write-Host "将以源码构建启动（当前分支：$branch）。" -ForegroundColor Yellow
Write-Host "注意：别去点已安装的 WanCode——那是旧版，测不到本分支的改动。" -ForegroundColor Yellow
Write-Host "自检：新建一个会话后，它的 summary.json 应当是" -ForegroundColor DarkGray
Write-Host "      current_model_id=glm-4.6（上游 slug）+ catalog_model_id=glm-open（配置键）；" -ForegroundColor DarkGray
Write-Host "      若 current 是 glm-open 且没有 catalog，说明跑的是旧版。" -ForegroundColor DarkGray
Write-Host ""
Write-Host "请核对这七项：" -ForegroundColor Cyan
Write-Host "  0. 应用启动后先『打开工作区』粘贴上面那个 proj 路径——"
Write-Host "     开错工作区的话，侧栏里根本不会出现下面那个会话。"
Write-Host "  1. 恢复上面那个旧会话 → 弹出选择器，列出两个候选，各自显示"
Write-Host "     真实端点（127.0.0.1:35101 与 :35102）。端点为空即为失败。"
Write-Host "  2. 此时发送按钮不可用。"
Write-Host "  3. 点『稍后再说』→ 弹窗收起，提示条仍在，发送仍不可用；"
Write-Host "     点提示条可重新展开。"
Write-Host "  4. 选『模拟·Coding Plan』→ 发一条消息 → 只有 CODING.requests.log"
Write-Host "     增加记录，OPEN.requests.log 不变。"
Write-Host "  5. 切走再恢复同一会话 → 不再询问（身份已写回）。"
Write-Host "  6. 切到一个新建的普通会话 → 弹窗与提示条完全消失。"
Write-Host "  7. 关掉应用，跑一次 -Step7（删掉全部 glm-4.6 条目），再恢复该会话 →"
Write-Host "     出现『模型已不在配置中』提示，从下拉另选 grok-build-solo 可解除。"
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
    Write-Host ""
    Write-Host "夹具保留在 $root（下次不带参数跑会重造；带 -Resume 沿用）" -ForegroundColor DarkGray
}
