function Write-WanCodeUpdateManifests {
  param(
    [Parameter(Mandatory = $true)][string]$Bundle,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Repo,
    [Parameter(Mandatory = $true)][string]$Mirror,
    [Parameter(Mandatory = $true)][string]$Signature,
    [Parameter(Mandatory = $true)][string]$PubDate
  )

  $originInstaller = "https://github.com/$Repo/releases/download/v$Version/wancode_${Version}_x64-setup.exe"
  $manifestFor = {
    param([string]$InstallerUrl)
    @{
      version   = $Version
      notes     = "WanCode v$Version"
      pub_date  = $PubDate
      platforms = @{
        "windows-x86_64" = @{
          signature = $Signature.Trim()
          url       = $InstallerUrl
        }
      }
    } | ConvertTo-Json -Depth 6
  }

  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  $originPath = Join-Path $Bundle "latest.json"
  $mirrorPath = Join-Path $Bundle "latest-gh-proxy.json"
  [System.IO.File]::WriteAllText($originPath, (& $manifestFor $originInstaller), $utf8NoBom)
  [System.IO.File]::WriteAllText($mirrorPath, (& $manifestFor "$Mirror$originInstaller"), $utf8NoBom)

  return @($originPath, $mirrorPath)
}
