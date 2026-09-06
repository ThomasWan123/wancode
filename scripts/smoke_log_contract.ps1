function Assert-SmokeLogContract {
  param(
    [Parameter(Mandatory = $true)][string]$LogPath,
    [Parameter(Mandatory = $true)][ValidateSet('full', 'work', 'c1-escape')][string]$ExpectedMode
  )

  $lines = @(Get-Content -LiteralPath $LogPath)
  $expect = @($lines | Where-Object { $_ -match '^SMOKE EXPECT mode=' })
  if ($expect.Count -ne 1) {
    throw "smoke contract: expected exactly one SMOKE EXPECT line, found $($expect.Count)"
  }
  if ($expect[0] -notmatch '^SMOKE EXPECT mode=(full|work|c1-escape) scenarios=([A-Za-z0-9,-]+)$') {
    throw "smoke contract: malformed expectation line: $($expect[0])"
  }
  $actualMode = $Matches[1]
  $scenarios = @($Matches[2] -split ',' | Where-Object { $_ -ne '' })
  if ($actualMode -ne $ExpectedMode) {
    throw "smoke contract: requested mode=$ExpectedMode but process reported mode=$actualMode"
  }
  if ($scenarios.Count -eq 0 -or @($scenarios | Select-Object -Unique).Count -ne $scenarios.Count) {
    throw "smoke contract: scenario set must be non-empty and unique"
  }
  foreach ($scenario in $scenarios) {
    $escaped = [regex]::Escape($scenario)
    $passLines = @($lines | Where-Object { $_ -match "^SMOKE SCENARIO $escaped PASS$" })
    if ($passLines.Count -ne 1) {
      throw "smoke contract: scenario $scenario expected one PASS, found $($passLines.Count)"
    }
  }

  $done = @($lines | Where-Object { $_ -match '^SMOKE DONE pass=\d+ fail=\d+$' })
  if ($done.Count -ne 1 -or $done[0] -notmatch '^SMOKE DONE pass=(\d+) fail=(\d+)$') {
    throw "smoke contract: expected exactly one well-formed SMOKE DONE line"
  }
  $pass = [int]$Matches[1]
  $fail = [int]$Matches[2]
  if ($fail -ne 0) { throw "smoke contract: $fail checks failed" }
  if ($pass -lt $scenarios.Count) {
    throw "smoke contract: pass count $pass is smaller than scenario count $($scenarios.Count)"
  }

  [pscustomobject]@{ Mode = $actualMode; Pass = $pass; Scenarios = $scenarios }
}
