# 迁移前后有效树等价审计——可由仓库/CI 重演的 materializer（#126 B1 复核定案 P0-1）
#
# 用法：powershell -File scripts/migration_audit.ps1 -BeforeSha <迁移前 wancode commit>
#                  [-Mode equivalent|intentional-delta|version-only|dependency-delta|wancode-lock-delta|security-upgrade-delta]
#                  [-Whitelist docs/design/v0.19-engine-file-whitelist.txt]
#                  [-EngineMirror ..\grok-build]
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
  [ValidateSet("equivalent", "intentional-delta", "version-only", "dependency-delta", "wancode-lock-delta", "security-upgrade-delta")][string]$Mode = "equivalent",
  [string]$Whitelist,
  [string]$EngineMirror,
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
  if ($EngineMirror) {
    $mirror = (Resolve-Path $EngineMirror).Path
    git -c core.longpaths=true -c core.autocrlf=false clone -q --shared --no-checkout $mirror $dest
    if ($LASTEXITCODE -ne 0) { throw "mirror clone 失败：$mirror" }
    git -C $dest checkout -q --detach $commit
  } else {
    # Fetch only the pinned commit. A full clone of the engine is both slow and
    # an unnecessary availability dependency for this byte-level audit.
    git -c core.longpaths=true -c core.autocrlf=false init -q $dest
    if ($LASTEXITCODE -ne 0) { throw "init 失败：$dest" }
    git -C $dest remote add origin $repo
    if ($LASTEXITCODE -ne 0) { throw "remote 配置失败：$repo" }
    git -C $dest -c protocol.version=2 fetch -q --depth=1 --no-tags origin $commit
    if ($LASTEXITCODE -ne 0) { throw "fetch 失败：$repo@$commit" }
    git -C $dest checkout -q --detach FETCH_HEAD
  }
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

# ── 非等价模式共用：收集有效树差异 ─────────────────────────────
$checks = [ordered]@{}
$failures = [System.Collections.Generic.List[string]]::new()
function Record-Check([string]$name, [bool]$ok, [string]$detail) {
  $script:checks[$name] = [ordered]@{ pass = $ok; detail = $detail }
  if (-not $ok) { $script:failures.Add("${name}: $detail") }
}

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

$beforeWiring = Join-Path $beforeDir "grok-build-wiring.patch"
$afterWiring = Join-Path $root "vendor\grok-build-wiring.patch"
$beforeCargo = Join-Path $beforeDir "grok-build-Cargo.lock"
$afterEmergency = Join-Path $root "vendor\grok-build-emergency.patch"
$afterWiringSha = Get-FileSha $afterWiring
$afterCargoSha = Get-FileSha $afterCargo
$beforeWiringSha = if (Test-Path $beforeWiring) { Get-FileSha $beforeWiring } else { "missing" }
$beforeCargoSha = Get-FileSha $beforeCargo

# ── dependency-delta：只允许 Cargo.lock 因**新增依赖**而变化 ────────
#
# 为什么需要第四种模式：加依赖这件事三种既有模式都表达不了——
#   equivalent       要求有效树逐字节相等（加依赖必然改 Cargo.lock）；
#   version-only     V3 只许 wancode 版本行不同（加依赖会新增 [[package]]）；
#   intentional-delta A1 要求引擎 commit **变**、A4 要求 lock **不变**，
#                    而加依赖恰恰相反（commit 不动、lock 动）。
# 硬套任何一种都只能靠放宽断言蒙混，那等于把门拆了。本模式逐项断言
# 「commit 没动、树里只有 lock 变了、lock 只多不改不减、多出来的恰好是
# 申报的那些、且没有引入原生链」。
if ($Mode -eq "dependency-delta") {
  # 解析 lock 的 name+version 集合（[[package]] 块）。
  function Read-LockPackages([string]$path) {
    $raw = [System.IO.File]::ReadAllText($path)
    $set = @{}
    $rx = "(?m)^\[\[package\]\]\s*$\s*^name = ""([^""]+)""\s*$\s*^version = ""([^""]+)""\s*$"
    foreach ($m in [regex]::Matches($raw, $rx)) {
      $set["$($m.Groups[1].Value) $($m.Groups[2].Value)"] = $true
    }
    if ($set.Count -eq 0) { throw "Cargo.lock 未解析出任何 package：$path" }
    return $set
  }
  $beforePkgs = Read-LockPackages $beforeCargo
  $afterPkgs  = Read-LockPackages $afterCargo
  # App releases also change the workspace member's own version. Keep that
  # orthogonal to dependency admission: filter only the wancode package from
  # the add/remove sets, then bind its exact version to tauri.conf below.
  $added   = @($afterPkgs.Keys  | Where-Object { -not $beforePkgs.ContainsKey($_) -and $_ -notmatch '^wancode ' } | Sort-Object)
  $removed = @($beforePkgs.Keys | Where-Object { -not $afterPkgs.ContainsKey($_) -and $_ -notmatch '^wancode ' }  | Sort-Object)

  function Read-DependencyDeltaWanCodeVersion([string]$path) {
    $raw = [System.IO.File]::ReadAllText($path)
    $rx = [regex]'(?ms)\[\[package\]\]\r?\nname = "wancode"\r?\nversion = "([^"]+)"'
    $matches = $rx.Matches($raw)
    if ($matches.Count -ne 1) { throw "Cargo.lock 中 wancode package 必须恰好一项：$path（实际 $($matches.Count)）" }
    return $matches[0].Groups[1].Value
  }
  $afterWanCodeVersion = Read-DependencyDeltaWanCodeVersion $afterCargo
  $appVersion = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version

  # 申报清单：清单里 declared_added_packages=「name version」逗号分隔。
  # 申报是为了让「多出来什么」成为评审对象，而不是藏在 118 行 diff 里。
  $declaredRaw = $afterBuildManifest.declared_added_packages
  $declared = @()
  if ($declaredRaw) { $declared = @($declaredRaw -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Sort-Object) }

  $onlyCargoLock = $changed.Count -eq 1 -and $changed[0].path -eq "Cargo.lock" -and $changed[0].change -eq "modified"
  $sysAdded = @($added | Where-Object { ($_ -split ' ')[0] -match '(?i)-sys$' })

  Record-Check "D1_engine_commit_unchanged" ($beforeCommit -eq $afterCommit) "before=$beforeCommit after=$afterCommit"
  Record-Check "D2_only_cargo_lock_changed" $onlyCargoLock $(if ($changed.Count) { (($changed | ForEach-Object { "$($_.change):$($_.path)" }) -join ', ') } else { "无有效树差异" })
  Record-Check "D3_no_packages_removed_or_downgraded" ($removed.Count -eq 0) $(if ($removed.Count) { "消失/改版: $($removed -join '; ')" } else { "既有 package 一个都没少、没改版" })
  Record-Check "D4_added_matches_declared" (($added -join '|') -ceq ($declared -join '|')) "added=[$($added -join '; ')] declared=[$($declared -join '; ')]"
  Record-Check "D5_no_native_sys_added" ($sysAdded.Count -eq 0) $(if ($sysAdded.Count) { "新增原生链: $($sysAdded -join '; ')" } else { "无 *-sys 新增（W1 教训，机器强制）" })
  Record-Check "D6_wiring_unchanged" ($beforeWiringSha -eq $afterWiringSha -and $afterBuildManifest.wiring_patch_sha256 -eq $afterWiringSha) "before=$beforeWiringSha after=$afterWiringSha manifest=$($afterBuildManifest.wiring_patch_sha256)"
  Record-Check "D7_hashes_registered" ($digestAfter -eq $afterBuildManifest.effective_tree_sha256 -and $afterBuildManifest.cargo_lock_sha256 -eq $afterCargoSha -and $afterBuildManifest.emergency_patch_sha256 -eq "none" -and (Get-Item $afterEmergency).Length -eq 0) "tree=$digestAfter manifest_tree=$($afterBuildManifest.effective_tree_sha256) lock=$afterCargoSha manifest_lock=$($afterBuildManifest.cargo_lock_sha256) emergency=$($afterBuildManifest.emergency_patch_sha256)"
  Record-Check "D8_wancode_version_matches_app" ($afterWanCodeVersion -eq $appVersion) "lock=$afterWanCodeVersion app=$appVersion"

  $summary = [ordered]@{
    mode                          = "dependency-delta"
    before_wancode_sha            = $before
    after_wancode_sha             = $after
    before_engine_commit          = $beforeCommit
    after_engine_commit           = $afterCommit
    added_packages                = $added
    declared_added_packages       = $declared
    removed_or_changed_packages   = $removed
    cargo_lock_sha256_before      = $beforeCargoSha
    cargo_lock_sha256_after       = $afterCargoSha
    before_effective_tree_sha256  = $digestBefore
    after_effective_tree_sha256   = $digestAfter
    changed_files                 = @($changed)
    checks                        = $checks
    pass                          = ($failures.Count -eq 0)
  }
  $summary | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $OutFile
  Write-Host "[migration-audit] dependency-delta 摘要已写 $OutFile"
  foreach ($name in $checks.Keys) {
    $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
    Write-Host "[migration-audit] $name=$mark — $($checks[$name].detail)"
  }
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
  if ($failures.Count) {
    Write-Host "MIGRATION AUDIT FAIL：dependency-delta 有 $($failures.Count) 项断言失败" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
  }
  Write-Host "MIGRATION AUDIT OK：dependency-delta 八项全 PASS（新增 $($added.Count) 个 package，无 *-sys，无既有 package 变动）"
  exit 0
}

# ── security-upgrade-delta：只允许 Cargo.lock 因**安全升级**而变化 ──
#
# 为什么需要第六种模式：升级既有依赖这件事前五种都表达不了——
#   equivalent        要求有效树逐字节相等；
#   version-only /
#   wancode-lock-delta 只许 wancode 自己那一块变；
#   dependency-delta  D3 明确要求「既有 package 一个没少、没改版」，
#                     而安全升级的定义就是既有 package 换版本，正好撞在 D3 上；
#   intentional-delta A1/A4 要求 commit 变、lock 不变，与此处恰好相反。
# 硬套只能靠放宽断言蒙混。本模式逐项断言「commit 没动、树里只有 lock 变了、
# 每一处版本变化都是**升级**（绝无降级或删除）、升级与顺带新增的包**逐条申报**、
# 每条升级绑一个 RUSTSEC 编号或显式标 collateral、且没有引入新的原生链」。
#
# 与 scripts/dependency_advisory_gate.ps1 的分工：那道门管「升级之后还剩哪些
# 命中、是否都已申报」，本模式管「这次 lock 变化本身是不是一次干净的升级」。
# 两者都绿才说明「依赖安全签核通过」，缺一不可。
if ($Mode -eq "security-upgrade-delta") {
  function Read-LockVersionMap([string]$path) {
    $raw = [System.IO.File]::ReadAllText($path)
    $map = @{}
    $rx = "(?m)^\[\[package\]\]\s*$\s*^name = ""([^""]+)""\s*$\s*^version = ""([^""]+)""\s*$"
    foreach ($m in [regex]::Matches($raw, $rx)) {
      $n = $m.Groups[1].Value
      if (-not $map.ContainsKey($n)) { $map[$n] = [System.Collections.Generic.List[string]]::new() }
      $map[$n].Add($m.Groups[2].Value)
    }
    if ($map.Count -eq 0) { throw "Cargo.lock 未解析出任何 package：$path" }
    return $map
  }
  # 语义化版本比较。不能用字符串比：'0.9.9' > '0.9.10' 会把降级判成升级。
  function Compare-SemVer([string]$a, [string]$b) {
    $pa = @(($a -split '[-+]')[0] -split '\.')
    $pb = @(($b -split '[-+]')[0] -split '\.')
    $n = [Math]::Max($pa.Count, $pb.Count)
    for ($i = 0; $i -lt $n; $i++) {
      $va = 0; $vb = 0
      if ($i -lt $pa.Count) { [void][int]::TryParse($pa[$i], [ref]$va) }
      if ($i -lt $pb.Count) { [void][int]::TryParse($pb[$i], [ref]$vb) }
      if ($va -ne $vb) { if ($va -gt $vb) { return 1 } else { return -1 } }
    }
    return 0
  }

  $beforeMapPkg = Read-LockVersionMap $beforeCargo
  $afterMapPkg = Read-LockVersionMap $afterCargo

  $upgrades = [System.Collections.Generic.List[string]]::new()   # "name from->to"
  $additions = [System.Collections.Generic.List[string]]::new()  # "name version"
  $regressions = [System.Collections.Generic.List[string]]::new()

  foreach ($name in ($afterMapPkg.Keys | Sort-Object)) {
    # wancode 自己的版本走 D8/W5 那套口径，不混进依赖升级账本。
    if ($name -eq "wancode") { continue }
    $bv = @()
    if ($beforeMapPkg.ContainsKey($name)) { $bv = @($beforeMapPkg[$name]) }
    $av = @($afterMapPkg[$name])
    $gone = @($bv | Where-Object { $av -notcontains $_ } | Sort-Object)
    $new = @($av | Where-Object { $bv -notcontains $_ } | Sort-Object)
    if ($gone.Count -eq 0 -and $new.Count -eq 0) { continue }
    if ($gone.Count -eq 0) {
      # 纯新增版本（老版本仍在树里）——按「新增」记账。
      foreach ($v in $new) { $additions.Add("$name $v") }
      continue
    }
    # 有版本消失：必须每一个都能配一个**更高**的新版本，且数量对齐。
    if ($gone.Count -ne $new.Count) {
      $regressions.Add("$name 消失 [$($gone -join ',')] 但新增 [$($new -join ',')]（数量对不上）")
      continue
    }
    for ($i = 0; $i -lt $gone.Count; $i++) {
      $from = $gone[$i]; $to = $new[$i]
      if ((Compare-SemVer $to $from) -le 0) {
        $regressions.Add("$name $from->$to 不是升级")
      } else {
        $upgrades.Add("$name $from->$to")
      }
    }
  }
  foreach ($name in ($beforeMapPkg.Keys | Sort-Object)) {
    if ($name -eq "wancode") { continue }
    if (-not $afterMapPkg.ContainsKey($name)) { $regressions.Add("$name 整个包名从 lock 里消失") }
  }

  # 申报清单（vendor/grok-build.lock 两行）：
  #   declared_security_upgrades = "name from->to RUSTSEC-nnnn-nnnn" 逗号分隔
  #   declared_upgrade_additions = "name version" 逗号分隔
  function Split-Declared([string]$raw) {
    if (-not $raw) { return @() }
    return @($raw -split ',' | ForEach-Object { ($_ -replace '\s+', ' ').Trim() } | Where-Object { $_ })
  }
  $declUpgradeRaw = Split-Declared $afterBuildManifest.declared_security_upgrades
  $declAdditions = @(Split-Declared $afterBuildManifest.declared_upgrade_additions | Sort-Object)

  # 每条升级申报三段：包名 + 旧版->新版 + 编号（RUSTSEC 或 collateral）
  $declUpgradePairs = [System.Collections.Generic.List[string]]::new()
  $badCitations = [System.Collections.Generic.List[string]]::new()
  $citedAdvisories = [System.Collections.Generic.List[string]]::new()
  foreach ($d in $declUpgradeRaw) {
    $parts = $d -split ' '
    if ($parts.Count -ne 3) { $badCitations.Add("「$d」应为『包名 旧版->新版 编号』三段"); continue }
    if ($parts[1] -notmatch '^[^>]+->[^>]+$') { $badCitations.Add("「$d」第二段应为 旧版->新版"); continue }
    # 一次升级可以同时修多条公告（quick-xml 0.41 就同时关掉 0194 和 0195），
    # 用 + 连接，别拆成两行——那会让 S4 的观察集合与申报集合对不上。
    if ($parts[2] -match '^RUSTSEC-\d{4}-\d{4}(\+RUSTSEC-\d{4}-\d{4})*$') {
      foreach ($id in ($parts[2] -split '\+')) { $citedAdvisories.Add($id) }
    } elseif ($parts[2] -ne "collateral") {
      # collateral = cargo 解析器为满足安全升级顺带抬上来的包，本身不修公告。
      # 允许它存在，但必须显式写出来，不许拿一个 RUSTSEC 号给一整批背书。
      $badCitations.Add("「$d」编号既不是 RUSTSEC-nnnn-nnnn（可用 + 连接多条）也不是 collateral：$($parts[2])")
      continue
    }
    $declUpgradePairs.Add("$($parts[0]) $($parts[1])")
  }
  $declUpgrades = @($declUpgradePairs | Sort-Object)
  $obsUpgrades = @($upgrades | Sort-Object)
  $obsAdditions = @($additions | Sort-Object)

  # 新增的**包名**才算引入原生链；既有 -sys 包多一个版本不算。
  $sysAddedNames = @($additions | ForEach-Object { ($_ -split ' ')[0] } |
    Where-Object { -not $beforeMapPkg.ContainsKey($_) -and $_ -match '(?i)-sys$' } | Sort-Object -Unique)

  $onlyCargoLock = $changed.Count -eq 1 -and $changed[0].path -eq "Cargo.lock" -and $changed[0].change -eq "modified"

  $rxWan = [regex]'(?ms)\[\[package\]\]\r?\nname = "wancode"\r?\nversion = "([^"]+)"'
  $wanMatches = $rxWan.Matches([System.IO.File]::ReadAllText($afterCargo))
  if ($wanMatches.Count -ne 1) { throw "Cargo.lock 中 wancode package 必须恰好一项（实际 $($wanMatches.Count)）" }
  $afterWanCodeVersion = $wanMatches[0].Groups[1].Value
  $appVersion = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version

  Record-Check "S1_engine_commit_unchanged" ($beforeCommit -eq $afterCommit) "before=$beforeCommit after=$afterCommit"
  Record-Check "S2_only_cargo_lock_changed" $onlyCargoLock $(if ($changed.Count) { (($changed | ForEach-Object { "$($_.change):$($_.path)" }) -join ', ') } else { "无有效树差异" })
  Record-Check "S3_no_downgrade_or_removal" ($regressions.Count -eq 0) $(if ($regressions.Count) { "降级/删除: $($regressions -join '; ')" } else { "$($upgrades.Count) 处版本变化全部是升级，无包被删" })
  Record-Check "S4_upgrades_match_declared" (($obsUpgrades -join '|') -ceq ($declUpgrades -join '|')) "observed=[$($obsUpgrades -join '; ')] declared=[$($declUpgrades -join '; ')]"
  Record-Check "S5_additions_match_declared" (($obsAdditions -join '|') -ceq ($declAdditions -join '|')) "observed=[$($obsAdditions -join '; ')] declared=[$($declAdditions -join '; ')]"
  Record-Check "S6_every_upgrade_cites_advisory_or_collateral" ($badCitations.Count -eq 0 -and $citedAdvisories.Count -gt 0) $(if ($badCitations.Count) { $badCitations -join '; ' } else { "引用公告 $(@($citedAdvisories | Sort-Object -Unique) -join ',')；其余标 collateral" })
  Record-Check "S7_no_native_sys_name_added" ($sysAddedNames.Count -eq 0) $(if ($sysAddedNames.Count) { "新增原生链包名: $($sysAddedNames -join '; ')" } else { "无新增 *-sys 包名（W1 教训）" })
  Record-Check "S8_wiring_unchanged" ($beforeWiringSha -eq $afterWiringSha -and $afterBuildManifest.wiring_patch_sha256 -eq $afterWiringSha) "before=$beforeWiringSha after=$afterWiringSha manifest=$($afterBuildManifest.wiring_patch_sha256)"
  Record-Check "S9_hashes_registered" ($digestAfter -eq $afterBuildManifest.effective_tree_sha256 -and $afterBuildManifest.cargo_lock_sha256 -eq $afterCargoSha -and $afterBuildManifest.emergency_patch_sha256 -eq "none" -and (Get-Item $afterEmergency).Length -eq 0) "tree=$digestAfter manifest_tree=$($afterBuildManifest.effective_tree_sha256) lock=$afterCargoSha manifest_lock=$($afterBuildManifest.cargo_lock_sha256)"
  Record-Check "S10_wancode_version_matches_app" ($afterWanCodeVersion -eq $appVersion) "lock=$afterWanCodeVersion app=$appVersion"

  $summary = [ordered]@{
    mode                         = "security-upgrade-delta"
    before_wancode_sha           = $before
    after_wancode_sha            = $after
    before_engine_commit         = $beforeCommit
    after_engine_commit          = $afterCommit
    upgrades                     = @($obsUpgrades)
    declared_security_upgrades   = @($declUpgradeRaw)
    additions                    = @($obsAdditions)
    declared_upgrade_additions   = @($declAdditions)
    regressions                  = @($regressions)
    cited_advisories             = @($citedAdvisories | Sort-Object -Unique)
    cargo_lock_sha256_before     = $beforeCargoSha
    cargo_lock_sha256_after      = $afterCargoSha
    before_effective_tree_sha256 = $digestBefore
    after_effective_tree_sha256  = $digestAfter
    changed_files                = @($changed)
    checks                       = $checks
    pass                         = ($failures.Count -eq 0)
  }
  $summary | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $OutFile
  Write-Host "[migration-audit] security-upgrade-delta 摘要已写 $OutFile"
  foreach ($name in $checks.Keys) {
    $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
    Write-Host "[migration-audit] $name=$mark — $($checks[$name].detail)"
  }
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
  if ($failures.Count) {
    Write-Host "MIGRATION AUDIT FAIL：security-upgrade-delta 有 $($failures.Count) 项断言失败" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
  }
  Write-Host "MIGRATION AUDIT OK：security-upgrade-delta 十项全 PASS（$($obsUpgrades.Count) 处升级，$($obsAdditions.Count) 处新增，无降级无删除）"
  exit 0
}

# ── release version-only：只允许 Cargo.lock 的 wancode 版本变化 ─
if ($Mode -eq "wancode-lock-delta") {
  function Read-WanCodeLockBlock([string]$path) {
    $raw = [System.IO.File]::ReadAllText($path)
    $rx = [regex]'(?ms)^\[\[package\]\]\r?\nname = "wancode"\r?\nversion = "([^"]+)"\r?\n(?<body>.*?)(?=^\[\[package\]\]|\z)'
    $matches = $rx.Matches($raw)
    if ($matches.Count -ne 1) { throw "Cargo.lock 中 wancode package 必须恰好一项：$path（实际 $($matches.Count)）" }
    $block = $matches[0]
    $deps = @([regex]::Matches($block.Groups['body'].Value, '(?m)^ "([^"]+)",?$') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    return [pscustomobject]@{
      version = $block.Groups[1].Value
      dependencies = $deps
      without_wancode = $raw.Remove($block.Index, $block.Length).Insert($block.Index, "<WANCODE_PACKAGE>`n")
    }
  }

  $beforeBlock = Read-WanCodeLockBlock $beforeCargo
  $afterBlock = Read-WanCodeLockBlock $afterCargo
  $addedDeps = @($afterBlock.dependencies | Where-Object { $_ -notin $beforeBlock.dependencies } | Sort-Object)
  $removedDeps = @($beforeBlock.dependencies | Where-Object { $_ -notin $afterBlock.dependencies } | Sort-Object)
  $declaredDeps = @()
  $declaredDepsRaw = $afterBuildManifest.declared_added_wancode_dependencies
  if ($declaredDepsRaw) { $declaredDeps = @($declaredDepsRaw -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Sort-Object -Unique) }
  $appVersion = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
  $onlyCargoLock = $changed.Count -eq 1 -and $changed[0].path -eq "Cargo.lock" -and $changed[0].change -eq "modified"

  Record-Check "W1_engine_commit_unchanged" ($beforeCommit -eq $afterCommit) "before=$beforeCommit after=$afterCommit"
  Record-Check "W2_only_cargo_lock_changed" $onlyCargoLock $(if ($changed.Count) { (($changed | ForEach-Object { "$($_.change):$($_.path)" }) -join ', ') } else { "无有效树差异" })
  Record-Check "W3_non_wancode_lock_identical" ($beforeBlock.without_wancode -ceq $afterBlock.without_wancode) "除 wancode package 块外 Cargo.lock 必须逐字节一致"
  Record-Check "W4_declared_dependencies_only" (($addedDeps -join '|') -ceq ($declaredDeps -join '|') -and $removedDeps.Count -eq 0) "added=[$($addedDeps -join '; ')] declared=[$($declaredDeps -join '; ')] removed=[$($removedDeps -join '; ')]"
  Record-Check "W5_version_changed_and_matches_app" ($beforeBlock.version -ne $afterBlock.version -and $afterBlock.version -eq $appVersion) "before=$($beforeBlock.version) after=$($afterBlock.version) app=$appVersion"
  Record-Check "W6_wiring_unchanged" ($beforeWiringSha -eq $afterWiringSha -and $afterBuildManifest.wiring_patch_sha256 -eq $afterWiringSha) "before=$beforeWiringSha after=$afterWiringSha manifest=$($afterBuildManifest.wiring_patch_sha256)"
  Record-Check "W7_hashes_registered" ($digestAfter -eq $afterBuildManifest.effective_tree_sha256 -and $afterBuildManifest.cargo_lock_sha256 -eq $afterCargoSha -and $afterBuildManifest.emergency_patch_sha256 -eq "none" -and (Get-Item $afterEmergency).Length -eq 0) "tree=$digestAfter manifest_tree=$($afterBuildManifest.effective_tree_sha256) lock=$afterCargoSha manifest_lock=$($afterBuildManifest.cargo_lock_sha256)"

  $summary = [ordered]@{
    mode = "wancode-lock-delta"
    before_wancode_sha = $before
    after_wancode_sha = $after
    before_version = $beforeBlock.version
    after_version = $afterBlock.version
    added_wancode_dependencies = $addedDeps
    declared_added_wancode_dependencies = $declaredDeps
    removed_wancode_dependencies = $removedDeps
    changed_files = @($changed)
    checks = $checks
    pass = ($failures.Count -eq 0)
  }
  $summary | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $OutFile
  Write-Host "[migration-audit] wancode-lock-delta 摘要已写 $OutFile"
  foreach ($name in $checks.Keys) {
    $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
    Write-Host "[migration-audit] $name=$mark — $($checks[$name].detail)"
  }
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
  if ($failures.Count) {
    Write-Host "MIGRATION AUDIT FAIL：wancode-lock-delta 有 $($failures.Count) 项断言失败" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
  }
  Write-Host "MIGRATION AUDIT OK：wancode-lock-delta 七项全 PASS（版本 + 已申报直接依赖）"
  exit 0
}

if ($Mode -eq "version-only") {
  function Read-WanCodeLockVersion([string]$path) {
    $raw = [System.IO.File]::ReadAllText($path)
    $rx = [regex]'(?ms)(\[\[package\]\]\r?\nname = "wancode"\r?\nversion = ")([^"]+)(")'
    $matches = $rx.Matches($raw)
    if ($matches.Count -ne 1) { throw "Cargo.lock 中 wancode package 必须恰好一项：$path（实际 $($matches.Count)）" }
    return [pscustomobject]@{
      version = $matches[0].Groups[2].Value
      normalized = $rx.Replace($raw, '${1}<WANCODE_VERSION>${3}')
    }
  }

  $beforeLock = Read-WanCodeLockVersion $beforeCargo
  $afterLock = Read-WanCodeLockVersion $afterCargo
  $appVersion = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
  $onlyCargoLock = $changed.Count -eq 1 -and $changed[0].path -eq "Cargo.lock" -and $changed[0].change -eq "modified"

  Record-Check "V1_engine_commit_unchanged" ($beforeCommit -eq $afterCommit) "before=$beforeCommit after=$afterCommit"
  Record-Check "V2_only_cargo_lock_changed" $onlyCargoLock $(if ($changed.Count) { (($changed | ForEach-Object { "$($_.change):$($_.path)" }) -join ', ') } else { "无有效树差异" })
  Record-Check "V3_lock_diff_is_only_wancode_version" ($beforeLock.normalized -ceq $afterLock.normalized -and $beforeLock.version -ne $afterLock.version) "before=$($beforeLock.version) after=$($afterLock.version)"
  Record-Check "V4_lock_version_matches_app" ($afterLock.version -eq $appVersion) "lock=$($afterLock.version) app=$appVersion"
  Record-Check "V5_wiring_unchanged" ($beforeWiringSha -eq $afterWiringSha -and $afterBuildManifest.wiring_patch_sha256 -eq $afterWiringSha) "before=$beforeWiringSha after=$afterWiringSha manifest=$($afterBuildManifest.wiring_patch_sha256)"
  Record-Check "V6_effective_tree_registered" ($digestAfter -eq $afterBuildManifest.effective_tree_sha256 -and $afterBuildManifest.cargo_lock_sha256 -eq $afterCargoSha) "tree=$digestAfter manifest_tree=$($afterBuildManifest.effective_tree_sha256) lock=$afterCargoSha manifest_lock=$($afterBuildManifest.cargo_lock_sha256)"
  Record-Check "V7_emergency_none" ($afterBuildManifest.emergency_patch_sha256 -eq "none" -and (Get-Item $afterEmergency).Length -eq 0) "manifest=$($afterBuildManifest.emergency_patch_sha256) bytes=$((Get-Item $afterEmergency).Length)"

  $summary = [ordered]@{
    mode                           = "version-only"
    before_wancode_sha            = $before
    after_wancode_sha             = $after
    before_engine_commit          = $beforeCommit
    after_engine_commit           = $afterCommit
    before_version                = $beforeLock.version
    after_version                 = $afterLock.version
    file_count_before             = $linesBefore.Count
    file_count_after              = $linesAfter.Count
    before_effective_tree_sha256  = $digestBefore
    after_effective_tree_sha256   = $digestAfter
    changed_files                 = @($changed)
    checks                        = $checks
    pass                          = ($failures.Count -eq 0)
  }
  $summary | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $OutFile
  Write-Host "[migration-audit] version-only 摘要已写 $OutFile"
  foreach ($name in $checks.Keys) {
    $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
    Write-Host "[migration-audit] $name=$mark — $($checks[$name].detail)"
  }
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
  if ($failures.Count) {
    Write-Host "MIGRATION AUDIT FAIL：version-only 有 $($failures.Count) 项断言失败" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
  }
  Write-Host "MIGRATION AUDIT OK：version-only 七项全 PASS（$($beforeLock.version) → $($afterLock.version)）"
  exit 0
}

# ── G26 intentional-delta：有意行为变化的白名单审计 ─────────────
$whitelistInfo = Read-DeltaWhitelist $Whitelist $treeBefore

$outside = @($changed | Where-Object { $_.change -ne "added" -and -not $whitelistInfo.allowed.Contains($_.path) })
$changedPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($entry in $changed) { [void]$changedPaths.Add($entry.path) }
$unused = @($whitelistInfo.allowed | Where-Object { -not $changedPaths.Contains($_) } | Sort-Object)
foreach ($path in $unused) { Write-Warning "白名单路径未改动：$path（须在 PR 描述说明）" }

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
