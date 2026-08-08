# 迁移前后有效树等价审计——可由仓库/CI 重演的 materializer（#126 B1 复核定案 P0-1）
#
# 用法：powershell -File scripts/migration_audit.ps1 -BeforeSha <迁移前 wancode commit>
#                  [-Mode equivalent|intentional-delta]
#                  [-Whitelist docs/design/v0.19-engine-file-whitelist.txt]
#                  [-OutFile migration-audit-summary.json]
#
# 语义：
#   before = git show <BeforeSha>:vendor/*  的 lock/patch/Cargo.lock 覆盖（旧输入钉死在 git 历史）
#   after  = 当前检出工作树的 vendor/*（本 PR 的新输入）
#   两侧都由【本脚本这同一个 materializer】构造临时树（clone raw 字节 → checkout →
#   固定顺序打 patch（空文件跳过）→ Cargo.lock 覆盖），不调用旧 commit 里的旧版 bootstrap。
#   然后规范化清单逐项比对（effective_tree_lib.ps1，与 verify 同源）。
#
# 输出：可留档 JSON 摘要（before/after wancode SHA、engine commit、各输入哈希、
#   文件数、两侧 effective_tree_sha256、equivalent），不等价即非零退出。
#
# 兼容两种 vendor 布局：旧 = grok-build-local.patch 单文件；
# 新 = grok-build-wiring.patch + grok-build-emergency.patch（B1 起）。
param(
  [Parameter(Mandatory)][string]$BeforeSha,
  [ValidateSet("equivalent", "intentional-delta")][string]$Mode = "equivalent",
  [string]$Whitelist,
  [string]$OutFile = "migration-audit-summary.json"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot "effective_tree_lib.ps1")

$before = (git -C $root rev-parse --verify "$BeforeSha^{commit}").Trim()
if ($LASTEXITCODE -ne 0) { throw "BeforeSha 无法解析：$BeforeSha" }
$after = (git -C $root rev-parse HEAD).Trim()

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("engine-migration-audit-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
Write-Host "[migration-audit] before=wancode@$($before.Substring(0,9)) after=wancode@$($after.Substring(0,9)) work=$work"

# git 历史里的旧输入必须按原始字节导出（PowerShell 管道会做字符串/编码转换，
# 用 cmd 重定向保证 byte-faithful）。
function Export-Blob([string]$sha, [string]$rel, [string]$dest) {
  cmd /c "git -C `"$root`" cat-file blob ${sha}:${rel} > `"$dest`"" | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "导出失败：${sha}:${rel}" }
}
function Test-BlobExists([string]$sha, [string]$rel) {
  # 经 cmd 吞掉 stderr：PS 5.1 下对 native 命令做 2>$null 重定向会把 fatal
  # 包装成 NativeCommandError，在 EAP=Stop 时直接终止脚本。
  cmd /c "git -C `"$root`" cat-file -e ${sha}:${rel} 2>nul" | Out-Null
  return ($LASTEXITCODE -eq 0)
}
function Read-RepoCommit([string]$lockFile) {
  $t = Get-Content $lockFile | Where-Object { $_ -match '^(repo|commit)=' }
  $repo = ($t | Where-Object { $_ -like 'repo=*' }) -replace '^repo=', ''
  $commit = ($t | Where-Object { $_ -like 'commit=*' }) -replace '^commit=', ''
  if (-not $repo -or -not $commit) { throw "lock 缺 repo=/commit=：$lockFile" }
  return @($repo, $commit)
}

function Read-DeltaWhitelist([string]$path, [string]$treeBefore) {
  if (-not $path) { throw "intentional-delta 模式需要 -Whitelist" }
  $resolved = (Resolve-Path $path).Path
  $allowed = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
  $sections = [ordered]@{ product = @(); tests = @() }
  $section = $null
  $lineNo = 0
  foreach ($raw in Get-Content $resolved) {
    $lineNo++
    $line = $raw.Trim()
    if (-not $line -or $line.StartsWith("#")) { continue }
    if ($line -match '^\[([^]]+)\]$') {
      $candidate = $Matches[1]
      if (-not $sections.Contains($candidate)) { throw "白名单未知段名 [$candidate]（$resolved`:$lineNo）" }
      $section = $candidate
      continue
    }
    if (-not $section) { throw "白名单条目不在段内（$resolved`:$lineNo）：$line" }
    $rel = $line -replace '\\', '/'
    if ([System.IO.Path]::IsPathRooted($rel) -or $rel -match '(^|/)\.\.(/|$)') {
      throw "白名单路径必须是仓库内相对路径（$resolved`:$lineNo）：$line"
    }
    if (-not $allowed.Add($rel)) { throw "白名单重复路径（段内或跨段）：$rel" }
    if (-not (Test-Path -LiteralPath (Join-Path $treeBefore ($rel -replace '/', '\\')))) {
      throw "白名单路径在 before 引擎树中不存在：$rel"
    }
    $sections[$section] += $rel
  }
  if (-not $allowed.Count) { throw "白名单为空：$resolved" }
  return [pscustomobject]@{ path = $resolved; allowed = $allowed; sections = $sections }
}

function Get-ManifestMap($lines) {
  $map = @{}
  foreach ($line in $lines) {
    $path, $hash = $line -split "`t", 2
    $map[$path] = $hash
  }
  return $map
}

# ── 收集两侧输入 ────────────────────────────────────────────────
$beforeDir = Join-Path $work "inputs-before"; New-Item -ItemType Directory -Path $beforeDir | Out-Null
Export-Blob $before "vendor/grok-build.lock" (Join-Path $beforeDir "grok-build.lock")
Export-Blob $before "vendor/grok-build-Cargo.lock" (Join-Path $beforeDir "grok-build-Cargo.lock")
$beforePatches = @()
if (Test-BlobExists $before "vendor/grok-build-wiring.patch") {
  Export-Blob $before "vendor/grok-build-wiring.patch" (Join-Path $beforeDir "grok-build-wiring.patch")
  Export-Blob $before "vendor/grok-build-emergency.patch" (Join-Path $beforeDir "grok-build-emergency.patch")
  $beforePatches = @((Join-Path $beforeDir "grok-build-wiring.patch"), (Join-Path $beforeDir "grok-build-emergency.patch"))
} else {
  Export-Blob $before "vendor/grok-build-local.patch" (Join-Path $beforeDir "grok-build-local.patch")
  $beforePatches = @((Join-Path $beforeDir "grok-build-local.patch"))
}
$beforeRepo, $beforeCommit = Read-RepoCommit (Join-Path $beforeDir "grok-build.lock")
$beforeBuildManifest = Read-BuildManifest (Join-Path $beforeDir "grok-build.lock")

$afterPatches = @((Join-Path $root "vendor\grok-build-wiring.patch"), (Join-Path $root "vendor\grok-build-emergency.patch"))
$afterCargo = Join-Path $root "vendor\grok-build-Cargo.lock"
$afterRepo, $afterCommit = Read-RepoCommit (Join-Path $root "vendor\grok-build.lock")
$afterBuildManifest = Read-BuildManifest (Join-Path $root "vendor\grok-build.lock")

# ── 同一个 materializer 构造两棵树 ─────────────────────────────
function New-EffectiveTree([string]$repo, [string]$commit, $patches, [string]$cargoLock, [string]$dest) {
  git clone -q -c core.longpaths=true -c core.autocrlf=false $repo $dest
  if ($LASTEXITCODE -ne 0) { throw "clone 失败：$repo" }
  git -C $dest checkout -q $commit
  if ($LASTEXITCODE -ne 0) { throw "checkout 失败：$commit" }
  foreach ($p in $patches) {
    if ((Get-Item $p).Length -gt 0) {
      git -C $dest apply $p
      if ($LASTEXITCODE -ne 0) { throw "patch 应用失败：$p" }
    }
  }
  Copy-Item $cargoLock (Join-Path $dest "Cargo.lock") -Force
}

$treeBefore = Join-Path $work "tree-before"
$treeAfter = Join-Path $work "tree-after"
New-EffectiveTree $beforeRepo $beforeCommit $beforePatches (Join-Path $beforeDir "grok-build-Cargo.lock") $treeBefore
New-EffectiveTree $afterRepo $afterCommit $afterPatches $afterCargo $treeAfter

# ── 规范化清单比对（与 verify 同源逻辑）─────────────────────────
$linesBefore = Get-NormalizedManifest $treeBefore
$linesAfter = Get-NormalizedManifest $treeAfter
$digestBefore = Get-ManifestDigest $linesBefore
$digestAfter = Get-ManifestDigest $linesAfter

if ($Mode -eq "equivalent") {
  $diffs = Compare-NormalizedManifests $linesAfter $linesBefore "after" "before"
  $equivalent = ($diffs -eq 0) -and ($digestBefore -eq $digestAfter)
  $summary = [ordered]@{
    mode                           = "equivalent"
    before_wancode_sha            = $before
    after_wancode_sha             = $after
    before_engine_commit          = $beforeCommit
    after_engine_commit           = $afterCommit
    before_inputs                 = [ordered]@{
      patches = @($beforePatches | ForEach-Object { [ordered]@{ file = (Split-Path $_ -Leaf); sha256 = (Get-FileSha $_) } })
      cargo_lock_sha256 = (Get-FileSha (Join-Path $beforeDir "grok-build-Cargo.lock"))
    }
    after_inputs                  = [ordered]@{
      patches = @($afterPatches | ForEach-Object { [ordered]@{ file = (Split-Path $_ -Leaf); sha256 = (Get-FileSha $_) } })
      cargo_lock_sha256 = (Get-FileSha $afterCargo)
    }
    file_count_before             = $linesBefore.Count
    file_count_after              = $linesAfter.Count
    before_effective_tree_sha256  = $digestBefore
    after_effective_tree_sha256   = $digestAfter
    equivalent                    = $equivalent
  }
  $summary | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $OutFile
  Write-Host "[migration-audit] 摘要已写 $OutFile"
  Write-Host ("[migration-audit] before={0} files, after={1} files" -f $linesBefore.Count, $linesAfter.Count)
  Write-Host "[migration-audit] before_effective_tree_sha256=$digestBefore"
  Write-Host "[migration-audit] after_effective_tree_sha256 =$digestAfter"
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
  if (-not $equivalent) {
    Write-Host "MIGRATION AUDIT FAIL：迁移前后有效树不等价（$diffs 项差异）" -ForegroundColor Red
    exit 1
  }
  Write-Host "MIGRATION AUDIT OK：迁移前后有效树逐字节等价（equivalent: true）"
  exit 0
}

# ── G26 intentional-delta：有意行为变化的白名单审计 ─────────────
$checks = [ordered]@{}
$failures = [System.Collections.Generic.List[string]]::new()
function Record-Check([string]$name, [bool]$ok, [string]$detail) {
  $script:checks[$name] = [ordered]@{ pass = $ok; detail = $detail }
  if (-not $ok) { $script:failures.Add("${name}: $detail") }
}

$whitelistInfo = Read-DeltaWhitelist $Whitelist $treeBefore
$beforeMap = Get-ManifestMap $linesBefore
$afterMap = Get-ManifestMap $linesAfter
$changed = [System.Collections.Generic.List[object]]::new()
$allPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($path in $beforeMap.Keys) { [void]$allPaths.Add($path) }
foreach ($path in $afterMap.Keys) { [void]$allPaths.Add($path) }
foreach ($path in ($allPaths | Sort-Object)) {
  $inBefore = $beforeMap.ContainsKey($path)
  $inAfter = $afterMap.ContainsKey($path)
  if ($inBefore -and $inAfter -and $beforeMap[$path] -eq $afterMap[$path]) { continue }
  $kind = if (-not $inBefore) { "added" } elseif (-not $inAfter) { "removed" } else { "modified" }
  $changed.Add([ordered]@{ path = $path; change = $kind })
}

$outside = @($changed | Where-Object { $_.change -ne "added" -and -not $whitelistInfo.allowed.Contains($_.path) })
$changedPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($entry in $changed) { [void]$changedPaths.Add($entry.path) }
$unused = @($whitelistInfo.allowed | Where-Object { -not $changedPaths.Contains($_) } | Sort-Object)
foreach ($path in $unused) { Write-Warning "白名单路径未改动：$path（须在 PR 描述说明）" }

$beforeWiring = Join-Path $beforeDir "grok-build-wiring.patch"
$afterWiring = Join-Path $root "vendor\grok-build-wiring.patch"
$beforeCargo = Join-Path $beforeDir "grok-build-Cargo.lock"
$afterEmergency = Join-Path $root "vendor\grok-build-emergency.patch"
$afterWiringSha = Get-FileSha $afterWiring
$afterCargoSha = Get-FileSha $afterCargo
$beforeWiringSha = if (Test-Path $beforeWiring) { Get-FileSha $beforeWiring } else { "missing" }
$beforeCargoSha = Get-FileSha $beforeCargo

Record-Check "A1_engine_commit_changed" ($beforeCommit -ne $afterCommit) "before=$beforeCommit after=$afterCommit"
Record-Check "A2_diff_within_whitelist" ($outside.Count -eq 0) $(if ($outside.Count) { ($outside.path -join ', ') } else { "$($changed.Count) 个差异文件均在白名单或为新增文件" })
Record-Check "A3_wiring_unchanged" ($beforeWiringSha -eq $afterWiringSha -and $afterBuildManifest.wiring_patch_sha256 -eq $afterWiringSha) "before=$beforeWiringSha after=$afterWiringSha manifest=$($afterBuildManifest.wiring_patch_sha256)"
Record-Check "A4_cargo_lock_unchanged" ($beforeCargoSha -eq $afterCargoSha -and $afterBuildManifest.cargo_lock_sha256 -eq $afterCargoSha) "before=$beforeCargoSha after=$afterCargoSha manifest=$($afterBuildManifest.cargo_lock_sha256)"
Record-Check "A5_effective_tree_registered" ($digestAfter -eq $afterBuildManifest.effective_tree_sha256) "computed=$digestAfter manifest=$($afterBuildManifest.effective_tree_sha256)"
Record-Check "A6_emergency_none" ($afterBuildManifest.emergency_patch_sha256 -eq "none" -and (Get-Item $afterEmergency).Length -eq 0) "manifest=$($afterBuildManifest.emergency_patch_sha256) bytes=$((Get-Item $afterEmergency).Length)"

$summary = [ordered]@{
  mode                           = "intentional-delta"
  approved_exception            = [ordered]@{
    design = "docs/design/v0.19-layered-surfaces.md §1.3"
    evidence = "docs/evidence/v019-2c-fanout-probe.log"
  }
  before_wancode_sha            = $before
  after_wancode_sha             = $after
  before_engine_commit          = $beforeCommit
  after_engine_commit           = $afterCommit
  file_count_before             = $linesBefore.Count
  file_count_after              = $linesAfter.Count
  before_effective_tree_sha256  = $digestBefore
  after_effective_tree_sha256   = $digestAfter
  changed_files                 = @($changed)
  whitelist                     = [ordered]@{
    file = $whitelistInfo.path
    product = @($whitelistInfo.sections.product)
    tests = @($whitelistInfo.sections.tests)
    unused = $unused
    outside = @($outside)
  }
  inputs                        = [ordered]@{
    wiring_patch_sha256_before = $beforeWiringSha
    wiring_patch_sha256_after = $afterWiringSha
    cargo_lock_sha256_before = $beforeCargoSha
    cargo_lock_sha256_after = $afterCargoSha
    emergency_patch_sha256_after = $afterBuildManifest.emergency_patch_sha256
  }
  checks                        = $checks
  pass                          = ($failures.Count -eq 0)
}
$summary | ConvertTo-Json -Depth 10 | Out-File -Encoding utf8 $OutFile
Write-Host "[migration-audit] intentional-delta 摘要已写 $OutFile"
foreach ($name in $checks.Keys) {
  $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
  Write-Host "[migration-audit] $name=$mark — $($checks[$name].detail)"
}
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
if ($failures.Count) {
  Write-Host "MIGRATION AUDIT FAIL：intentional-delta 有 $($failures.Count) 项断言失败" -ForegroundColor Red
  $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
  exit 1
}
Write-Host "MIGRATION AUDIT OK：intentional-delta 六项全 PASS（$($changed.Count) 个差异文件）"
