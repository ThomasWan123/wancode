# 依赖公告门（v0.20 复核 P1）：把 `cargo audit` 的实际命中集合与
# `docs/security/dependency-advisory-exemptions.txt` 的申报集合**双向**比对。
#
# 为什么不用 `cargo audit --ignore`：`--ignore` 按公告号整条静音，同一个公告
# 日后命中到别的包/别的版本/别的链路一样不响，等于把门开在了公告粒度上。
# 本门绑 (公告号, 包名, 版本) 三元组，且双向断言——命中未申报要红，申报已
# 不再命中（僵尸豁免）也要红，逼着修复落地时同步删登记。
#
# 口径：只拦 `vulnerabilities`。`unmaintained` / `unsound` / `yanked` 归
# `warnings`，本门只统计不拦截——把"警告很多"和"有已知漏洞"分开说。
param(
  [string]$Lock = "vendor/grok-build-Cargo.lock",
  [string]$Exemptions = "docs/security/dependency-advisory-exemptions.txt",
  [string]$OutFile = "dependency-advisory-summary.json",
  # 离线/夹具用：不 fetch 公告库。CI 不许带，否则门可能拿陈旧库判绿。
  [switch]$NoFetch,
  # 夹具用：直接喂一份 cargo audit --json 输出，不真的跑 cargo。
  [string]$AuditJson = ""
)
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$lockPath = if ([System.IO.Path]::IsPathRooted($Lock)) { $Lock } else { Join-Path $root $Lock }
$exPath = if ([System.IO.Path]::IsPathRooted($Exemptions)) { $Exemptions } else { Join-Path $root $Exemptions }

$VALID_REASONS = @("not-linked-windows", "lock-orphan", "engine-pinned-g26", "no-upstream-fix")

$checks = [ordered]@{}
$failures = [System.Collections.Generic.List[string]]::new()
function Record-Check([string]$name, [bool]$ok, [string]$detail) {
  $script:checks[$name] = [ordered]@{ pass = $ok; detail = $detail }
  if (-not $ok) { $script:failures.Add("${name}: $detail") }
}

# ── 取 cargo audit 的 JSON ────────────────────────────────────
# cargo audit 有命中时退出码为 1——那是正常输出，不是执行失败，所以这里
# 不看退出码，只看 JSON 是否可解析（A1 负责）。用 Start-Process 重定向而不是
# `2>&1`：PS 5.1 会把原生 stderr 包成 ErrorRecord，干净的一次运行也会被判失败。
$auditRaw = ""
$auditExit = -1
if ($AuditJson) {
  $ajPath = if ([System.IO.Path]::IsPathRooted($AuditJson)) { $AuditJson } else { Join-Path $root $AuditJson }
  $auditRaw = [System.IO.File]::ReadAllText($ajPath)
  $auditExit = 0
} else {
  $argList = @("audit", "--json", "--color", "never", "-f", $lockPath)
  if ($NoFetch) { $argList += "-n" }
  $tmpOut = [System.IO.Path]::GetTempFileName()
  $tmpErr = [System.IO.Path]::GetTempFileName()
  try {
    $proc = Start-Process -FilePath "cargo" -ArgumentList $argList -NoNewWindow -Wait -PassThru `
      -RedirectStandardOutput $tmpOut -RedirectStandardError $tmpErr
    $auditExit = $proc.ExitCode
    $auditRaw = [System.IO.File]::ReadAllText($tmpOut)
    $auditErr = [System.IO.File]::ReadAllText($tmpErr)
    if (-not $auditRaw.Trim()) { Write-Host "[dep-advisory] cargo audit 无 stdout；stderr：`n$auditErr" }
  } finally {
    Remove-Item $tmpOut, $tmpErr -Force -ErrorAction SilentlyContinue
  }
}

$audit = $null
$parseErr = ""
try { $audit = $auditRaw | ConvertFrom-Json } catch { $parseErr = $_.Exception.Message }

# 不只验证「JSON 能解析」：还要把 cargo-audit 的关键 schema 绑死。否则输出若
# 被截断/版本漂移成 { vulnerabilities: { found: true, count: 1 } }，`list`
# 缺失会在 PowerShell 里被当成空数组；等豁免清零后，这种坏输出会误判为零漏洞。
$schemaOk = $false
$schemaDetail = ""
if ($null -ne $audit -and $null -ne $audit.vulnerabilities) {
  $vp = $audit.vulnerabilities.PSObject.Properties
  $hasFound = $null -ne $vp["found"]
  $hasCount = $null -ne $vp["count"]
  $hasList = $null -ne $vp["list"]
  if ($hasFound -and $hasCount -and $hasList) {
    $listCount = @($audit.vulnerabilities.list).Count
    $reportedCount = -1
    $countParsed = [int]::TryParse("$($audit.vulnerabilities.count)", [ref]$reportedCount)
    $reportedFound = $audit.vulnerabilities.found -eq $true
    $schemaOk = $countParsed -and $reportedCount -eq $listCount -and $reportedFound -eq ($listCount -gt 0)
    $schemaDetail = "found=$reportedFound count=$reportedCount list_count=$listCount"
  } else {
    $schemaDetail = "缺字段：found=$hasFound count=$hasCount list=$hasList"
  }
}

Record-Check "A1_audit_output_parsed" ($null -ne $audit -and $schemaOk) `
  $(if ($null -eq $audit) { "cargo audit --json 无法解析：$parseErr" } elseif (-not $schemaOk) { "cargo audit --json schema 不一致：$schemaDetail" } else { "exit=$auditExit dependencies=$($audit.lockfile.'dependency-count') $schemaDetail" })

if ($failures.Count) {
  # 拿不到命中集合就没法做任何比对——继续跑只会产出"看起来全绿"的假摘要。
  $summary = [ordered]@{ lock = $Lock; checks = $checks; pass = $false }
  $summary | ConvertTo-Json -Depth 10 | Out-File -Encoding utf8 $OutFile
  Write-Host "DEPENDENCY ADVISORY FAIL：$($failures[0])" -ForegroundColor Red
  exit 1
}

# ── 实际命中集合 ──────────────────────────────────────────────
$observed = [System.Collections.Generic.List[string]]::new()
$observedRows = [System.Collections.Generic.List[object]]::new()
foreach ($v in @($audit.vulnerabilities.list)) {
  $key = "$($v.advisory.id) $($v.package.name) $($v.package.version)"
  $observed.Add($key)
  $patched = @($v.versions.patched) -join ","
  $observedRows.Add([ordered]@{
      advisory = $v.advisory.id
      package  = $v.package.name
      version  = $v.package.version
      patched  = $patched
    })
}

# ── 申报集合 ──────────────────────────────────────────────────
$declared = [System.Collections.Generic.List[string]]::new()
$declaredRows = [System.Collections.Generic.List[object]]::new()
$malformed = [System.Collections.Generic.List[string]]::new()
$declaredKeys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
$lineNo = 0
foreach ($line in [System.IO.File]::ReadAllLines($exPath)) {
  $lineNo++
  $t = $line.Trim()
  if (-not $t -or $t.StartsWith("#")) { continue }
  $f = $t -split '\s+'
  if ($f.Count -ne 5) { $malformed.Add("第 $lineNo 行应为 5 列，实得 $($f.Count)：$t"); continue }
  if ($f[0] -notmatch '^RUSTSEC-\d{4}-\d{4}$') { $malformed.Add("第 $lineNo 行公告号格式错：$($f[0])"); continue }
  if ($VALID_REASONS -notcontains $f[3]) { $malformed.Add("第 $lineNo 行理由码不在固定集合内：$($f[3])"); continue }
  $parsedDate = [datetime]::MinValue
  if (-not [datetime]::TryParseExact($f[4], 'yyyy-MM-dd', [cultureinfo]::InvariantCulture, [System.Globalization.DateTimeStyles]::None, [ref]$parsedDate)) {
    $malformed.Add("第 $lineNo 行复核日期格式错（要 YYYY-MM-DD）：$($f[4])"); continue
  }
  if ($parsedDate.Date -gt [datetime]::UtcNow.Date) {
    $malformed.Add("第 $lineNo 行复核日期在未来：$($f[4])"); continue
  }
  $declaredKey = "$($f[0]) $($f[1]) $($f[2])"
  if (-not $declaredKeys.Add($declaredKey)) {
    $malformed.Add("第 $lineNo 行重复申报：$declaredKey"); continue
  }
  $declared.Add($declaredKey)
  $declaredRows.Add([ordered]@{ advisory = $f[0]; package = $f[1]; version = $f[2]; reason = $f[3]; reviewed = $f[4] })
}

$undeclared = @($observed | Where-Object { $declared -notcontains $_ } | Sort-Object)
$stale = @($declared | Where-Object { $observed -notcontains $_ } | Sort-Object)

Record-Check "A2_exemption_file_wellformed" ($malformed.Count -eq 0) `
  $(if ($malformed.Count) { $malformed -join "; " } else { "$($declaredRows.Count) 条申报，五列齐备、理由码合法、日期可解析" })
Record-Check "A3_no_undeclared_vulnerability" ($undeclared.Count -eq 0) `
  $(if ($undeclared.Count) { "未申报命中：$($undeclared -join '; ')" } else { "$($observed.Count) 条命中全部已申报" })
Record-Check "A4_no_stale_exemption" ($stale.Count -eq 0) `
  $(if ($stale.Count) { "僵尸豁免（已不再命中，应删）：$($stale -join '; ')" } else { "无僵尸条目" })
Record-Check "A5_lock_is_the_vendored_engine_lock" ($audit.lockfile.'dependency-count' -gt 0) `
  "审计对象=$Lock dependency_count=$($audit.lockfile.'dependency-count')"

$summary = [ordered]@{
  lock                  = $Lock
  exemptions_file       = $Exemptions
  advisory_db_commit    = $audit.database.'last-commit'
  advisory_count        = $audit.database.'advisory-count'
  dependency_count      = $audit.lockfile.'dependency-count'
  vulnerabilities_found = $observedRows.Count
  vulnerabilities       = @($observedRows)
  exemptions            = @($declaredRows)
  undeclared            = @($undeclared)
  stale_exemptions      = @($stale)
  # 只统计不拦截，见文件头口径说明。
  warnings_count        = @($audit.warnings.PSObject.Properties | ForEach-Object { @($_.Value).Count } | Measure-Object -Sum).Sum
  checks                = $checks
  pass                  = ($failures.Count -eq 0)
}
$summary | ConvertTo-Json -Depth 10 | Out-File -Encoding utf8 $OutFile
Write-Host "[dep-advisory] 摘要已写 $OutFile"
foreach ($name in $checks.Keys) {
  $mark = if ($checks[$name].pass) { "PASS" } else { "FAIL" }
  Write-Host "[dep-advisory] $name=$mark — $($checks[$name].detail)"
}
if ($failures.Count) {
  Write-Host "DEPENDENCY ADVISORY FAIL：$($failures.Count) 项断言失败" -ForegroundColor Red
  $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
  exit 1
}
Write-Host "DEPENDENCY ADVISORY OK：$($observedRows.Count) 条命中全部已申报，无僵尸豁免"
