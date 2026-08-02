# 有效树等价审计 + 构建清单校验（#126 B1，设计稿 §3/§4/§5）
#
# 规范化清单：排除 .git/、target/ 与审计临时文件（*.audit-tmp），
# 对其余全部文件计算「相对路径<TAB>sha256」，按路径排序；
# 清单整体（UTF-8、LF 连接）再 sha256 = effective_tree_sha256。
# 相对路径统一正斜杠。Cargo.lock 属工作树，照常入列。
#
# 模式：
#   hash    -Tree <dir>                打印该树的 effective_tree_sha256
#   compare -TreeA <dir> -TreeB <dir>  两树规范化清单逐项比对，差异全打印
#   verify  -Engine <dir>              CI 三道断言（设计稿 §4/§5）：
#                                      ① 引擎 HEAD == 清单 commit
#                                      ② wiring/emergency/cargo_lock 三文件哈希 == 清单；
#                                        emergency=none ⇔ 0 字节；非空则头部须含
#                                        事故编号+到期版本，当前版本 ≥ 到期版本即 fail
#                                      ③ porcelain 精确集合 == patch 触及 ∪ {Cargo.lock}
#                                        （快速结构检查）+ 复算 effective_tree_sha256 == 清单
param(
  [Parameter(Mandatory, Position = 0)][ValidateSet("hash", "compare", "verify")][string]$Mode,
  [string]$Tree,
  [string]$TreeA,
  [string]$TreeB,
  [string]$Engine
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent

function Get-NormalizedManifest([string]$dir) {
  $dir = (Resolve-Path $dir).Path
  $lines = [System.Collections.Generic.List[string]]::new()
  Get-ChildItem -Path $dir -Recurse -File -Force | ForEach-Object {
    $rel = $_.FullName.Substring($dir.Length).TrimStart('\', '/') -replace '\\', '/'
    if ($rel -like '.git/*' -or $rel -like 'target/*' -or $rel -like '*/target/*' -or $rel -like '*.audit-tmp') { return }
    $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
    $lines.Add("$rel`t$h")
  }
  $lines.Sort([System.StringComparer]::Ordinal)
  return $lines
}

function Get-ManifestDigest($lines) {
  $bytes = [System.Text.Encoding]::UTF8.GetBytes(($lines -join "`n") + "`n")
  $sha = [System.Security.Cryptography.SHA256]::Create()
  return ([System.BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').ToLowerInvariant()
}

function Get-BuildManifest {
  $m = @{}
  Get-Content (Join-Path $root "vendor\grok-build.lock") | Where-Object { $_ -match '^[a-z0-9_]+=' } | ForEach-Object {
    $k, $v = $_ -split '=', 2
    $m[$k] = $v
  }
  foreach ($k in "repo", "commit", "wiring_patch_sha256", "emergency_patch_sha256", "cargo_lock_sha256", "effective_tree_sha256") {
    if (-not $m[$k]) { throw "构建清单缺字段：$k" }
  }
  return $m
}

function Get-FileSha([string]$path) {
  return (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
}

switch ($Mode) {
  "hash" {
    if (-not $Tree) { throw "hash 模式需要 -Tree" }
    Write-Output (Get-ManifestDigest (Get-NormalizedManifest $Tree))
  }
  "compare" {
    if (-not $TreeA -or -not $TreeB) { throw "compare 模式需要 -TreeA 与 -TreeB" }
    $a = Get-NormalizedManifest $TreeA
    $b = Get-NormalizedManifest $TreeB
    $mapA = @{}; foreach ($l in $a) { $p, $h = $l -split "`t"; $mapA[$p] = $h }
    $mapB = @{}; foreach ($l in $b) { $p, $h = $l -split "`t"; $mapB[$p] = $h }
    $bad = 0
    foreach ($p in $mapA.Keys) {
      if (-not $mapB.ContainsKey($p)) { Write-Host "只在 A：$p"; $bad++ }
      elseif ($mapA[$p] -ne $mapB[$p]) { Write-Host "内容不同：$p"; $bad++ }
    }
    foreach ($p in $mapB.Keys) { if (-not $mapA.ContainsKey($p)) { Write-Host "只在 B：$p"; $bad++ } }
    if ($bad -gt 0) { Write-Host "AUDIT FAIL：$bad 项差异（A=$TreeA B=$TreeB）" -ForegroundColor Red; exit 1 }
    Write-Host "AUDIT OK：两树逐字节等价（$($a.Count) 文件）；effective_tree_sha256=$(Get-ManifestDigest $a)"
  }
  "verify" {
    if (-not $Engine) { throw "verify 模式需要 -Engine" }
    $m = Get-BuildManifest
    # ① HEAD == commit
    $head = (git -C $Engine rev-parse HEAD).Trim()
    if ($head -ne $m.commit) { Write-Host "VERIFY FAIL：引擎 HEAD=$head != 清单 commit=$($m.commit)" -ForegroundColor Red; exit 1 }
    # ② 三文件内容哈希
    $wiring = Join-Path $root "vendor\grok-build-wiring.patch"
    if ((Get-FileSha $wiring) -ne $m.wiring_patch_sha256) { Write-Host "VERIFY FAIL：wiring patch 哈希不符" -ForegroundColor Red; exit 1 }
    $emerg = Join-Path $root "vendor\grok-build-emergency.patch"
    if ($m.emergency_patch_sha256 -eq "none") {
      if ((Get-Item $emerg).Length -ne 0) { Write-Host "VERIFY FAIL：清单声明 none 但 emergency patch 非 0 字节" -ForegroundColor Red; exit 1 }
    } else {
      if ((Get-Item $emerg).Length -eq 0) { Write-Host "VERIFY FAIL：清单登记哈希但 emergency patch 为空" -ForegroundColor Red; exit 1 }
      if ((Get-FileSha $emerg) -ne $m.emergency_patch_sha256) { Write-Host "VERIFY FAIL：emergency patch 哈希不符" -ForegroundColor Red; exit 1 }
      $header = Get-Content $emerg -TotalCount 10
      $incident = ($header | Select-String '^# incident:\s*(\S+)').Matches.Groups | Select-Object -Last 1
      $expiry = ($header | Select-String '^# expires_in_version:\s*(\d+\.\d+\.\d+)').Matches.Groups | Select-Object -Last 1
      if (-not $incident -or -not $expiry) { Write-Host "VERIFY FAIL：emergency patch 头部缺事故编号或到期版本" -ForegroundColor Red; exit 1 }
      $cur = [version]((Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version)
      if ($cur -ge [version]$expiry.Value) { Write-Host "VERIFY FAIL：emergency patch 已到期（当前 $cur ≥ 到期 $($expiry.Value)），必须清偿" -ForegroundColor Red; exit 1 }
      Write-Host "emergency patch 在期：事故 $($incident.Value)，$cur < $($expiry.Value)" -ForegroundColor Yellow
    }
    $overlay = Join-Path $root "vendor\grok-build-Cargo.lock"
    if ((Get-FileSha $overlay) -ne $m.cargo_lock_sha256) { Write-Host "VERIFY FAIL：Cargo.lock 覆盖文件哈希不符" -ForegroundColor Red; exit 1 }
    # ③ porcelain 精确集合（快速结构检查）+ 有效树摘要复算
    $expected = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($patch in @($wiring) + $(if ($m.emergency_patch_sha256 -ne "none") { @($emerg) } else { @() })) {
      Select-String '^diff --git a/(.+) b/' -Path $patch | ForEach-Object { [void]$expected.Add($_.Matches.Groups[1].Value) }
    }
    [void]$expected.Add("Cargo.lock")
    $actual = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    git -C $Engine status --porcelain | ForEach-Object { [void]$actual.Add(($_.Substring(3).Trim('"'))) }
    if (-not $expected.SetEquals($actual)) {
      Write-Host "VERIFY FAIL：porcelain 集合与 patch 触及 ∪ {Cargo.lock} 不符" -ForegroundColor Red
      Write-Host ("  期望: " + (($expected | Sort-Object) -join ", "))
      Write-Host ("  实际: " + (($actual | Sort-Object) -join ", "))
      exit 1
    }
    $digest = Get-ManifestDigest (Get-NormalizedManifest $Engine)
    if ($digest -ne $m.effective_tree_sha256) { Write-Host "VERIFY FAIL：effective_tree_sha256 复算=$digest != 清单=$($m.effective_tree_sha256)" -ForegroundColor Red; exit 1 }
    Write-Host "VERIFY OK：HEAD/三文件哈希/porcelain 集合/有效树摘要 全部匹配（engine=$($m.commit.Substring(0,9))）"
  }
}
