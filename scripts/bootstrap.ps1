# WanCode 一键开发环境搭建（v0.13-3）
#
# 用法：  powershell -File scripts/bootstrap.ps1
#
# 做四件事：
#   1. 检查工具链（rustup/cargo、protoc、node/npm、git；MSVC 由 rustup target 隐含）
#   2. 在仓库【兄弟目录】../grok-build 克隆引擎并 checkout vendor/grok-build.lock
#      指定的 commit（引擎必须是兄弟目录：src-tauri/Cargo.toml 靠
#      workspace = "../../grok-build" 吃依赖继承，见 vendor/grok-build.lock 头注）
#   3. 应用 vendor/grok-build-local.patch（Windows protoc 修复 + workspace 成员）
#   4. npm install
#
# 幂等：grok-build 已存在则只校验 commit 与补丁状态，不动本地改动。
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent          # wancode 仓库根
$parent = Split-Path $root -Parent                # 引擎的兄弟层
$engine = Join-Path $parent "grok-build"

# ── 1. 工具链检查 ────────────────────────────────────────────────
$missing = @()
foreach ($tool in "git", "cargo", "node", "npm") {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { $missing += $tool }
}
$protoc = Join-Path $env:USERPROFILE ".protoc\bin\protoc.exe"
if (-not (Test-Path $protoc) -and -not (Get-Command protoc -ErrorAction SilentlyContinue)) {
  $missing += "protoc（建议解压到 %USERPROFILE%\.protoc）"
}
if ($missing.Count -gt 0) {
  Write-Host "[bootstrap] 缺少工具：$($missing -join '、')" -ForegroundColor Red
  Write-Host "  rustup: https://rustup.rs （含 MSVC target；另需 VS2022 C++ 生成工具 + LLVM 组件提供 lld-link）"
  Write-Host "  protoc: https://github.com/protocolbuffers/protobuf/releases"
  exit 1
}

# ── 2. 读取 lock 并准备引擎目录 ─────────────────────────────────
$lock = Get-Content (Join-Path $root "vendor\grok-build.lock") | Where-Object { $_ -match '^(repo|commit)=' }
$repo = ($lock | Where-Object { $_ -like 'repo=*' }) -replace '^repo=', ''
$commit = ($lock | Where-Object { $_ -like 'commit=*' }) -replace '^commit=', ''
if (-not $repo -or -not $commit) { throw "vendor/grok-build.lock 缺 repo=/commit= 行" }

if (-not (Test-Path $engine)) {
  Write-Host "[bootstrap] clone $repo -> $engine @ $($commit.Substring(0,9))"
  # core.longpaths：pager 快照文件名超 Windows 260 字符限制，不开会 checkout 失败
  git clone -c core.longpaths=true $repo $engine
  if ($LASTEXITCODE -ne 0) { throw "clone 失败" }
  Push-Location $engine
  git checkout $commit
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "checkout $commit 失败" }
  # ── 3. 打本地补丁 + 锁定依赖解析 ──
  git apply (Join-Path $root "vendor\grok-build-local.patch")
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "补丁应用失败（vendor/grok-build-local.patch）" }
  # 覆盖 Cargo.lock：wancode 挂进 workspace 后依赖树被扩展过，
  # 用 vendor 里冻结的解析结果，避免新机器重解析出不同小版本。
  Copy-Item (Join-Path $root "vendor\grok-build-Cargo.lock") "Cargo.lock" -Force
  Pop-Location
  Write-Host "[bootstrap] 引擎就绪（含 Windows protoc 补丁）"
} else {
  Push-Location $engine
  $head = git rev-parse HEAD
  $patched = (git status --short -- Cargo.toml) -ne $null
  Pop-Location
  if ($head -ne $commit) {
    Write-Host "[bootstrap] 警告：../grok-build HEAD=$($head.Substring(0,9)) 与 lock=$($commit.Substring(0,9)) 不一致" -ForegroundColor Yellow
    Write-Host "           升级引擎请自行 checkout 后重打补丁并跑全量 smoke。"
  } elseif (-not $patched) {
    Write-Host "[bootstrap] 警告：引擎 commit 正确但本地补丁似未应用（Cargo.toml 无改动）" -ForegroundColor Yellow
  } else {
    Write-Host "[bootstrap] ../grok-build 已就绪（commit 与补丁均匹配），跳过"
  }
}

# ── 4. 前端依赖 ─────────────────────────────────────────────────
Push-Location $root
npm install
Pop-Location
if ($LASTEXITCODE -ne 0) { throw "npm install 失败" }

Write-Host ""
Write-Host "[bootstrap] 完成。下一步："
Write-Host "  构建调试版： cd src-tauri; cargo build -p wancode   （环境变量见 scripts/smoke.ps1 头部）"
Write-Host "  引擎冒烟：   powershell -File scripts/smoke.ps1"
Write-Host "  发布：       powershell -File scripts/release.ps1"
