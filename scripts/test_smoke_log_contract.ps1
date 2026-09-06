$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'smoke_log_contract.ps1')

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("wancode-smoke-contract-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $testRoot | Out-Null
try {
  function Write-Case([string]$Name, [string[]]$Lines) {
    $path = Join-Path $testRoot "$Name.log"
    Set-Content -LiteralPath $path -Value $Lines -Encoding utf8
    $path
  }
  function Must-Fail([scriptblock]$Action, [string]$Name) {
    try { & $Action; throw "negative control unexpectedly passed: $Name" }
    catch {
      if ($_.Exception.Message -like 'negative control unexpectedly passed:*') { throw }
    }
  }

  $full = Write-Case 'full' @(
    'SMOKE BEGIN',
    'SMOKE EXPECT mode=full scenarios=S1-start,S2-reply',
    'SMOKE SCENARIO S1-start PASS',
    'SMOKE SCENARIO S2-reply PASS',
    'SMOKE DONE pass=2 fail=0'
  )
  $result = Assert-SmokeLogContract -LogPath $full -ExpectedMode full
  if ($result.Mode -ne 'full' -or $result.Pass -ne 2) { throw 'positive full contract failed' }

  $subset = Write-Case 'subset' @(
    'SMOKE EXPECT mode=work scenarios=S7-work',
    'SMOKE SCENARIO S7-work PASS',
    'SMOKE DONE pass=10 fail=0'
  )
  $result = Assert-SmokeLogContract -LogPath $subset -ExpectedMode work
  if ($result.Mode -ne 'work' -or $result.Pass -ne 10) { throw 'positive subset contract failed' }

  $missing = Write-Case 'missing' @(
    'SMOKE EXPECT mode=full scenarios=S1-start,S2-reply',
    'SMOKE SCENARIO S1-start PASS',
    'SMOKE DONE pass=1 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $missing -ExpectedMode full } 'missing scenario'

  $wrongMode = Write-Case 'wrong-mode' @(
    'SMOKE EXPECT mode=work scenarios=S7-work',
    'SMOKE SCENARIO S7-work PASS',
    'SMOKE DONE pass=10 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $wrongMode -ExpectedMode full } 'inherited subset mode'

  $zero = Write-Case 'zero' @('SMOKE EXPECT mode=full scenarios=S1-start', 'SMOKE DONE pass=0 fail=0')
  Must-Fail { Assert-SmokeLogContract -LogPath $zero -ExpectedMode full } 'zero-check pass'

  $extra = Write-Case 'extra' @(
    'SMOKE EXPECT mode=work scenarios=S7-work',
    'SMOKE SCENARIO S7-work PASS',
    'SMOKE SCENARIO S2-reply PASS',
    'SMOKE DONE pass=11 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $extra -ExpectedMode work } 'unexpected scenario'
  Write-Host 'smoke log contract positive and negative controls passed'
} finally {
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
