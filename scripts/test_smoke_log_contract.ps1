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
  function Must-Fail([scriptblock]$Action, [string]$Name, [string]$ExpectedMessage) {
    try { & $Action; throw "negative control unexpectedly passed: $Name" }
    catch {
      if ($_.Exception.Message -like 'negative control unexpectedly passed:*') { throw }
      if ($_.Exception.Message -notlike $ExpectedMessage) {
        throw "negative control failed for the wrong reason: $Name; expected '$ExpectedMessage', got '$($_.Exception.Message)'"
      }
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
  Must-Fail { Assert-SmokeLogContract -LogPath $missing -ExpectedMode full } `
    'missing scenario' 'smoke contract: reported scenario set does not exactly match expectation'

  $wrongMode = Write-Case 'wrong-mode' @(
    'SMOKE EXPECT mode=work scenarios=S7-work',
    'SMOKE SCENARIO S7-work PASS',
    'SMOKE DONE pass=10 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $wrongMode -ExpectedMode full } `
    'inherited subset mode' 'smoke contract: requested mode=full but process reported mode=work'

  $zero = Write-Case 'zero' @(
    'SMOKE EXPECT mode=full scenarios=S1-start',
    'SMOKE SCENARIO S1-start PASS',
    'SMOKE DONE pass=0 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $zero -ExpectedMode full } `
    'zero-check pass' 'smoke contract: pass count 0 is smaller than scenario count 1'

  $extra = Write-Case 'extra' @(
    'SMOKE EXPECT mode=work scenarios=S7-work',
    'SMOKE SCENARIO S7-work PASS',
    'SMOKE SCENARIO S2-reply PASS',
    'SMOKE DONE pass=11 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $extra -ExpectedMode work } `
    'unexpected scenario' 'smoke contract: reported scenario set does not exactly match expectation'

  $failedScenario = Write-Case 'failed-scenario' @(
    'SMOKE EXPECT mode=full scenarios=S1-start',
    'SMOKE SCENARIO S1-start FAIL',
    'SMOKE DONE pass=0 fail=1'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $failedScenario -ExpectedMode full } `
    'failed scenario' 'smoke contract: scenario S1-start did not pass'

  $missingExpect = Write-Case 'missing-expect' @(
    'SMOKE SCENARIO S1-start PASS',
    'SMOKE DONE pass=1 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $missingExpect -ExpectedMode full } `
    'missing expectation' 'smoke contract: expected exactly one SMOKE EXPECT line, found 0'

  $duplicateExpect = Write-Case 'duplicate-expect' @(
    'SMOKE EXPECT mode=full scenarios=S1-start',
    'SMOKE EXPECT mode=full scenarios=S1-start',
    'SMOKE SCENARIO S1-start PASS',
    'SMOKE DONE pass=1 fail=0'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $duplicateExpect -ExpectedMode full } `
    'duplicate expectation' 'smoke contract: expected exactly one SMOKE EXPECT line, found 2'

  $missingDone = Write-Case 'missing-done' @(
    'SMOKE EXPECT mode=full scenarios=S1-start',
    'SMOKE SCENARIO S1-start PASS'
  )
  Must-Fail { Assert-SmokeLogContract -LogPath $missingDone -ExpectedMode full } `
    'missing completion' 'smoke contract: expected exactly one well-formed SMOKE DONE line'
  Write-Host 'smoke log contract positive and negative controls passed'
} finally {
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
