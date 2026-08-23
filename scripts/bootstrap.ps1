# WanCode 一键开发环境搭建（#126 B1：-Dest 参数化 + wiring/emergency 双补丁）
#
# 用法：  powershell -File scripts/bootstrap.ps1 [-Dest <引擎目录>]
#
# 做四件事：
#   1. 检查工具链（rustup/cargo、protoc、node/npm、git；MSVC 由 rustup target 隐含）
#   2. 按 vendor/grok-build.lock（构建清单）克隆引擎并 checkout 固定 commit。
#      缺省目录 = 仓库【兄弟目录】../grok-build（src-tauri/Cargo.toml 靠
#      workspace = "../../grok-build" 吃依赖继承）；-Dest 供审计脚本同机产多棵树。
#   3. 打补丁，固定顺序：先 vendor/grok-build-wiring.patch（常驻接线 + 迁移期残留）；
#      vendor/grok-build-emergency.patch 仅在非空时应用（git apply 对空输入必报错，
#      空即跳过是执行语义的一部分）。随后覆盖 Cargo.lock。
#   4. npm install（仅缺省目录时；-Dest 审计树跳过前端）
#
# 幂等：目标已存在则只校验 commit 与补丁状态，不动本地改动。
param(
  [string]$Dest
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent          # wancode 仓库根
$isDefaultDest = -not $Dest
if ($isDefaultDest) {
  $parent = Split-Path $root -Parent              # 引擎的兄弟层
  $engine = Join-Path $parent "grok-build"
} else {
  $engine = $Dest
}

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

# ── 2. 读取构建清单并准备引擎目录 ───────────────────────────────
$lock = Get-Content (Join-Path $root "vendor\grok-build.lock") | Where-Object { $_ -match '^(repo|commit)=' }
$repo = ($lock | Where-Object { $_ -like 'repo=*' }) -replace '^repo=', ''
$commit = ($lock | Where-Object { $_ -like 'commit=*' }) -replace '^commit=', ''
if (-not $repo -or -not $commit) { throw "vendor/grok-build.lock 缺 repo=/commit= 行" }

if (-not (Test-Path $engine)) {
  Write-Host "[bootstrap] clone $repo -> $engine @ $($commit.Substring(0,9))"
  # core.longpaths：pager 快照文件名超 Windows 260 字符限制，不开会 checkout 失败。
  # core.autocrlf=false：有效树摘要（effective_tree_sha256）要求字节跨机器确定，
  # 工作树必须是原始 blob 字节，不做任何 EOL 转换（vendor 补丁也以 LF 存，见 .gitattributes）。
  # Do not let clone materialize the remote default branch first. On Windows,
  # that first checkout can inherit host EOL policy and a later checkout of the
  # pinned commit leaves unchanged files byte-dirty. Materialize exactly once,
  # with the policy supplied to both Git processes.
  git -c core.longpaths=true -c core.autocrlf=false clone --no-checkout $repo $engine
  if ($LASTEXITCODE -ne 0) { throw "clone 失败" }
  Push-Location $engine
  git -c core.autocrlf=false -C $engine checkout --detach $commit
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "checkout $commit 失败" }
  # ── 3. 打补丁（固定顺序 wiring → emergency）+ 锁定依赖解析 ──
  git apply (Join-Path $root "vendor\grok-build-wiring.patch")
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "补丁应用失败（vendor/grok-build-wiring.patch）" }
  $emerg = Join-Path $root "vendor\grok-build-emergency.patch"
  if ((Get-Item $emerg).Length -gt 0) {
    Write-Host "[bootstrap] 应用紧急补丁 vendor/grok-build-emergency.patch" -ForegroundColor Yellow
    git apply $emerg
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw "补丁应用失败（vendor/grok-build-emergency.patch）" }
  }
  # 覆盖 Cargo.lock：wancode 挂进 workspace 后依赖树被扩展过，
  # 用 vendor 里冻结的解析结果，避免新机器重解析出不同小版本。
  Copy-Item (Join-Path $root "vendor\grok-build-Cargo.lock") "Cargo.lock" -Force
  Pop-Location
} else {
  Write-Host "[bootstrap] $engine 已存在，进入 verify 级校验（不做任何自动修改）"
}

# ── 3.5 就绪门（#126 B1 复核定案）：无论新 clone 还是已有目录，宣布 ready 前
# 必须过 verify 级完整校验（HEAD/三文件哈希/porcelain 精确集合/有效树摘要）。
# 任一不符：报错 + 非零退出 + 不执行 npm install；绝不自动删除或覆盖已有目录。
powershell -NoProfile -File (Join-Path $PSScriptRoot "audit_effective_tree.ps1") verify -Engine $engine
if ($LASTEXITCODE -ne 0) {
  Write-Host "[bootstrap] 引擎目录未通过构建清单校验，判为【非就绪】。" -ForegroundColor Red
  Write-Host "  安全处置（本脚本不会自动删改该目录）：" -ForegroundColor Red
  Write-Host "    1) 若目录内无你的手工改动：把它移走（如改名 grok-build-old）后重跑 bootstrap，"
  Write-Host "       会按构建清单重新 clone 出规范化树（raw 字节，禁 EOL 转换）；"
  Write-Host "    2) 或用 -Dest 在别处创建规范化树，再自行切换；"
  Write-Host "    3) 注意：老树若曾以 autocrlf=true 物化（CRLF），仅改配置无法修复，必须重 clone。"
  exit 1
}
Write-Host "[bootstrap] 引擎就绪（fork@$($commit.Substring(0,9)) + wiring patch，verify 全过）"

if (-not $isDefaultDest) {
  Write-Host "[bootstrap] -Dest 模式：跳过 npm install（审计树只需引擎工作树）"
  exit 0
}

# ── 4. 前端依赖 ─────────────────────────────────────────────────
Push-Location $root
npm install
Pop-Location
if ($LASTEXITCODE -ne 0) { throw "npm install 失败" }

Write-Host ""
Write-Host "[bootstrap] 完成。下一步："
Write-Host "  构建调试版： cd src-tauri; cargo build --locked -p wancode   （环境变量见 scripts/smoke.ps1 头部）"
Write-Host "  引擎冒烟：   powershell -File scripts/smoke.ps1"
Write-Host "  发布：       powershell -File scripts/release.ps1"
