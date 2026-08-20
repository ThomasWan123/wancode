# 依赖公告门的负向门测试：证明 fail-closed 在每种错误状态下真的会红，
# 且错误原因文本命中——不接受"反正失败了"。
#
# 用合成夹具（一份最小 cargo audit --json + 一份申报文件）驱动
# dependency_advisory_gate.ps1 的 -AuditJson 通道，不跑真 cargo：
# 门的逻辑是集合比对，真跑 cargo 只会把测试变慢并绑到当天的公告库上。
$ErrorActionPreference = "Stop"
$gate = Join-Path $PSScriptRoot "dependency_advisory_gate.ps1"

$fx = Join-Path ([System.IO.Path]::GetTempPath()) ("dep-advisory-negative-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $fx | Out-Null

function W([string]$path, [string]$content) {
  [System.IO.File]::WriteAllText($path, $content.Replace("`r`n", "`n"), [System.Text.UTF8Encoding]::new($false))
}

# 两条命中的最小 audit JSON。字段名与 cargo audit --json 一致
# （lockfile 用连字符键 `dependency-count`，踩过一次，别改回下划线）。
function Audit-Json([string[]]$rows) {
  $items = foreach ($r in $rows) {
    $p = $r -split '\s+'
    @"
{"advisory":{"id":"$($p[0])"},"versions":{"patched":[]},"package":{"name":"$($p[1])","version":"$($p[2])"}}
"@
  }
  return @"
{"database":{"advisory-count":1,"last-commit":null},"lockfile":{"dependency-count":42},
 "vulnerabilities":{"found":true,"count":$($rows.Count),"list":[$($items -join ',')]},
 "warnings":{}}
"@
}

$HITS = @("RUSTSEC-2026-0194 quick-xml 0.38.3", "RUSTSEC-2023-0071 rsa 0.9.10")
$GOOD_DECL = @"
# 夹具申报文件
RUSTSEC-2026-0194  quick-xml  0.38.3  engine-pinned-g26  2026-08-20
RUSTSEC-2023-0071  rsa        0.9.10  no-upstream-fix    2026-08-20
"@

$auditOk = Join-Path $fx "audit-ok.json"
W $auditOk (Audit-Json $HITS)

$cases = @(
  @{ name = "正向对照：命中与申报一一对应"; decl = $GOOD_DECL; audit = $auditOk; expectPass = $true; needle = "A3_no_undeclared_vulnerability=PASS" },
  @{ name = "未申报命中必须红"; decl = "RUSTSEC-2026-0194  quick-xml  0.38.3  engine-pinned-g26  2026-08-20"; audit = $auditOk; expectPass = $false; needle = "未申报命中" },
  @{ name = "版本对不上也算未申报（不许按公告号整条静音）"; decl = @"
RUSTSEC-2026-0194  quick-xml  0.37.5  engine-pinned-g26  2026-08-20
RUSTSEC-2023-0071  rsa        0.9.10  no-upstream-fix    2026-08-20
"@; audit = $auditOk; expectPass = $false; needle = "未申报命中" },
  @{ name = "僵尸豁免（已不再命中）必须红"; decl = @"
$GOOD_DECL
RUSTSEC-2026-0257  webbrowser  1.0.6  no-upstream-fix  2026-08-20
"@; audit = $auditOk; expectPass = $false; needle = "僵尸豁免" },
  @{ name = "列数不对必须红"; decl = "RUSTSEC-2026-0194  quick-xml  0.38.3  engine-pinned-g26"; audit = $auditOk; expectPass = $false; needle = "应为 5 列" },
  @{ name = "理由码不在固定集合必须红"; decl = @"
RUSTSEC-2026-0194  quick-xml  0.38.3  whatever-i-feel-like  2026-08-20
RUSTSEC-2023-0071  rsa        0.9.10  no-upstream-fix       2026-08-20
"@; audit = $auditOk; expectPass = $false; needle = "理由码不在固定集合内" },
  @{ name = "复核日期格式错必须红"; decl = @"
RUSTSEC-2026-0194  quick-xml  0.38.3  engine-pinned-g26  2026/08/20
RUSTSEC-2023-0071  rsa        0.9.10  no-upstream-fix    2026-08-20
"@; audit = $auditOk; expectPass = $false; needle = "复核日期格式错" },
  @{ name = "公告号格式错必须红"; decl = @"
RUSTSEC-26-0194    quick-xml  0.38.3  engine-pinned-g26  2026-08-20
RUSTSEC-2023-0071  rsa        0.9.10  no-upstream-fix    2026-08-20
"@; audit = $auditOk; expectPass = $false; needle = "公告号格式错" }
)

# audit 输出不可解析 → 必须红在 A1，且**不许**继续往下判绿
$auditBad = Join-Path $fx "audit-bad.json"
W $auditBad "not json at all"
$cases += @{ name = "audit 输出不可解析必须红在 A1"; decl = $GOOD_DECL; audit = $auditBad; expectPass = $false; needle = "无法解析" }

$failed = 0
$i = 0
foreach ($c in $cases) {
  $i++
  $declPath = Join-Path $fx "decl-$i.txt"
  W $declPath $c.decl
  $out = Join-Path $fx "summary-$i.json"
  $log = & powershell -NoProfile -File $gate -Exemptions $declPath -AuditJson $c.audit -OutFile $out
  $code = $LASTEXITCODE
  $text = ($log | Out-String)
  $okCode = if ($c.expectPass) { $code -eq 0 } else { $code -ne 0 }
  $okNeedle = $text -match [regex]::Escape($c.needle)
  if ($okCode -and $okNeedle) {
    Write-Host "  PASS  $($c.name)"
  } else {
    $failed++
    Write-Host "  FAIL  $($c.name)（exit=$code 期望$(if ($c.expectPass) { '=0' } else { '≠0' })，原因文本命中=$okNeedle）" -ForegroundColor Red
    Write-Host $text
  }
}

Remove-Item -Recurse -Force $fx -ErrorAction SilentlyContinue
if ($failed) {
  Write-Host "负向门测试 FAIL：$failed/$($cases.Count) 个场景不符合预期" -ForegroundColor Red
  exit 1
}
Write-Host "负向门测试 OK：$($cases.Count)/$($cases.Count) 个场景（含 1 个正向对照）符合预期"
