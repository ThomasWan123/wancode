# 按 vendor/pdfium.lock 取回并**校验** PDFium 原生二进制（来源政策 2）。
#
#   fetch_pdfium.ps1            取回 + 校验 + 解包到 vendor/pdfium-runtime/
#   fetch_pdfium.ps1 -VerifyOnly  只校验已存在的产物（CI 复核用，不联网）
#
# fail-closed：归档 sha256 或 dll sha256 与清单不符 → 立即失败、删除产物、
# 非零退出。绝不"先用着"——供应链校验一旦允许降级就等于没有。
#
# 二进制不入库（政策 2）：产物落 vendor/pdfium-runtime/，该目录被 .gitignore。
param([switch]$VerifyOnly)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$lockPath = Join-Path $root "vendor\pdfium.lock"
$outDir = Join-Path $root "vendor\pdfium-runtime"

# ── 读清单（key=value，忽略注释/空行）──────────────────────────────
$m = @{}
foreach ($line in Get-Content $lockPath) {
  if ($line -match '^\s*#' -or $line -match '^\s*$') { continue }
  $kv = $line -split '=', 2
  if ($kv.Count -eq 2) { $m[$kv[0].Trim()] = $kv[1].Trim() }
}
foreach ($k in @('release_tag','win_x64_url','win_x64_archive_sha256','win_x64_dll_path','win_x64_dll_sha256','win_x64_dll_bytes')) {
  if (-not $m.ContainsKey($k)) { Write-Host "FETCH FAIL：清单缺字段 $k" -ForegroundColor Red; exit 1 }
}
# 禁用 latest —— 清单被人改成浮动 tag 时必须当场失败。
if ($m['release_tag'] -match '(?i)latest') {
  Write-Host "FETCH FAIL：release_tag 不得为 latest（供应链必须钉死）" -ForegroundColor Red; exit 1
}
if ($m['win_x64_url'] -notmatch [regex]::Escape($m['release_tag'])) {
  Write-Host "FETCH FAIL：URL 与 release_tag 不一致（$($m['release_tag'])）" -ForegroundColor Red; exit 1
}
# 无 V8 守卫：v8 变体内嵌 JS 引擎，解析不受信文档时是额外攻击面。
if ($m['win_x64_asset'] -match '(?i)v8') {
  Write-Host "FETCH FAIL：不得使用 v8 变体（内嵌 JS 引擎 = 额外攻击面）" -ForegroundColor Red; exit 1
}

$dllPath = Join-Path $outDir ($m['win_x64_dll_path'] -replace '/', '\')

function Test-Artifact {
  if (-not (Test-Path $dllPath)) { return $false }
  $h = (Get-FileHash $dllPath -Algorithm SHA256).Hash.ToLower()
  $sz = (Get-Item $dllPath).Length
  if ($h -ne $m['win_x64_dll_sha256']) {
    Write-Host "VERIFY FAIL：pdfium.dll sha256 不符" -ForegroundColor Red
    Write-Host "  清单=$($m['win_x64_dll_sha256'])"; Write-Host "  实得=$h"
    return $false
  }
  if ($sz -ne [int64]$m['win_x64_dll_bytes']) {
    Write-Host "VERIFY FAIL：pdfium.dll 体积不符（清单=$($m['win_x64_dll_bytes']) 实得=$sz）" -ForegroundColor Red
    return $false
  }
  return $true
}

if ($VerifyOnly) {
  if (Test-Artifact) {
    Write-Host "VERIFY OK：pdfium.dll sha256/体积与清单一致（$($m['pdfium_version']), $($m['win_x64_dll_bytes']) B）"
    exit 0
  }
  Write-Host "VERIFY FAIL：产物缺失或不符——先跑 fetch_pdfium.ps1" -ForegroundColor Red
  exit 1
}

if (Test-Artifact) {
  Write-Host "已就位且校验通过，跳过下载：$dllPath"
  exit 0
}

# ── 取回 ───────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force $outDir | Out-Null
$tmp = Join-Path $outDir $m['win_x64_asset']
Remove-Item $tmp -EA SilentlyContinue
Write-Host "[pdfium] 取回 $($m['win_x64_url'])"
Invoke-WebRequest -Uri $m['win_x64_url'] -OutFile $tmp -UseBasicParsing

# 归档校验必须在解包**之前**——不校验就解包等于让未验证的归档写文件系统。
$ah = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
if ($ah -ne $m['win_x64_archive_sha256']) {
  Remove-Item $tmp -Force -EA SilentlyContinue
  Write-Host "FETCH FAIL：归档 sha256 不符（已删除产物）" -ForegroundColor Red
  Write-Host "  清单=$($m['win_x64_archive_sha256'])"; Write-Host "  实得=$ah"
  exit 1
}
Write-Host "[pdfium] 归档校验通过，解包"
tar -xzf $tmp -C $outDir
Remove-Item $tmp -Force -EA SilentlyContinue

if (-not (Test-Artifact)) {
  Remove-Item $outDir -Recurse -Force -EA SilentlyContinue
  Write-Host "FETCH FAIL：解包后 dll 校验不通过（已清理）" -ForegroundColor Red
  exit 1
}
$lic = Join-Path $outDir $m['license_file']
if (-not (Test-Path $lic)) {
  Write-Host "FETCH FAIL：缺许可证文件 $($m['license_file'])——再分发必须随附" -ForegroundColor Red
  exit 1
}
Write-Host "FETCH OK：$($m['pdfium_version']) / $($m['win_x64_dll_bytes']) B / 许可证 $($m['wrapper_license']) + $($m['pdfium_license'])"
