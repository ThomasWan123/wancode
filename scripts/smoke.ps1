# WanCode 引擎级 smoke 套件（v0.13 重构安全网）
#
# 用法：  pwsh -File scripts/smoke.ps1 [-SkipBuild]
#
# 6 个场景（会话启动/基本回复/忙时排队/回合插话/git 状态+贮藏/会话恢复）
# 全部走真实引擎与真实模型 API，断言落在磁盘与 git2 层——不碰 UI 坐标。
# 结果：%TEMP%\wancode-autotest.log，进程退出码 0=全过。
#
# 前置：~/.grok 已配置模型（smoke 用默认模型；消耗少量 API 调用）。
param([switch]$SkipBuild)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent

# 工具链环境（与 release.ps1 一致）
$env:Path = "$env:Path;$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.protoc\bin;C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin"
$env:PROTOC = "$env:USERPROFILE\.protoc\bin\protoc.exe"
$env:RUSTFLAGS = "-C link-arg=/STACK:16777216"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "lld-link"

if (-not $SkipBuild) {
  Write-Host "[smoke] cargo build -p wancode ..."
  Set-Location "$root\src-tauri"
  cargo build -p wancode
  if ($LASTEXITCODE -ne 0) { throw "build 失败" }
}

# 引擎在仓库兄弟目录（见 vendor/grok-build.lock），产物落引擎 workspace 的 target
$exe = Join-Path (Split-Path $root -Parent) "grok-build\target\debug\wancode.exe"
if (-not (Test-Path $exe)) { throw "找不到 $exe" }

# 一次性 fixture 工作区
$fixture = Join-Path $env:TEMP ("wancode-smoke-" + (Get-Date -Format "HHmmss"))
New-Item -ItemType Directory -Force $fixture | Out-Null
Set-Content -Path (Join-Path $fixture "notes.md") -Value "smoke fixture" -Encoding utf8

Get-Process wancode -EA SilentlyContinue | Stop-Process -Force -Confirm:$false
$log = Join-Path $env:TEMP "wancode-autotest.log"
Remove-Item $log -EA SilentlyContinue

# 进程 cwd 必须在任何 git 仓库之外——引擎存在按进程 cwd 回退的路径（#83 同族）
Set-Location $env:TEMP
Write-Host "[smoke] launching with WANCODE_AUTOTEST=$fixture"
$env:WANCODE_AUTOTEST = $fixture
$stderr = Join-Path $env:TEMP "wancode-smoke-stderr.log"
Remove-Item $stderr -EA SilentlyContinue
$proc = Start-Process -FilePath $exe -PassThru -RedirectStandardError $stderr
$env:WANCODE_AUTOTEST = $null

# 轮询日志直到 SMOKE DONE（上限 8 分钟——含多次真实模型回合）
$deadline = (Get-Date).AddMinutes(8)
$done = $false
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 5
  if ((Test-Path $log) -and (Select-String -Path $log -Pattern "SMOKE DONE" -Quiet)) { $done = $true; break }
  if ($proc.HasExited) { Write-Host "[smoke] 应用中途退出 exit=$($proc.ExitCode)"; break }
}

Write-Host "──── wancode-autotest.log ────"
if (Test-Path $log) { Get-Content $log } else { Write-Host "(无日志——启动失败？)" }
Write-Host "──────────────────────────────"
if (Test-Path $stderr) { Write-Host "──── stderr ────"; Get-Content $stderr -Tail 20; Write-Host "────────────────" }

Get-Process wancode -EA SilentlyContinue | Stop-Process -Force -Confirm:$false
Remove-Item -Recurse -Force $fixture -EA SilentlyContinue

if (-not $done) { Write-Host "[smoke] 超时或未完成"; exit 1 }
$fail = (Select-String -Path $log -Pattern "SMOKE DONE pass=\d+ fail=(\d+)").Matches[0].Groups[1].Value
if ([int]$fail -gt 0) { Write-Host "[smoke] FAIL x$fail"; exit 1 }
Write-Host "[smoke] ALL PASS"
exit 0
