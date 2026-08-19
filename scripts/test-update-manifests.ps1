$ErrorActionPreference = "Stop"
. "$PSScriptRoot/update-manifests.ps1"

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("wancode-manifest-test-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $root | Out-Null
try {
  $paths = Write-WanCodeUpdateManifests `
    -Bundle $root `
    -Version "0.20.0" `
    -Repo "ThomasWan123/wancode" `
    -Mirror "https://gh-proxy.com/" `
    -Signature "test-signature" `
    -PubDate "2026-08-19T00:00:00Z"

  if ($paths.Count -ne 2) { throw "必须生成两份 manifest" }
  $origin = Get-Content (Join-Path $root "latest.json") -Raw | ConvertFrom-Json
  $mirror = Get-Content (Join-Path $root "latest-gh-proxy.json") -Raw | ConvertFrom-Json
  $originPlatform = $origin.platforms."windows-x86_64"
  $mirrorPlatform = $mirror.platforms."windows-x86_64"

  if ($origin.version -ne "0.20.0" -or $mirror.version -ne $origin.version) {
    throw "双 manifest 版本不一致"
  }
  if ($originPlatform.signature -ne "test-signature" -or $mirrorPlatform.signature -ne $originPlatform.signature) {
    throw "双 manifest 未绑定同一安装包签名"
  }
  if ($originPlatform.url -ne "https://github.com/ThomasWan123/wancode/releases/download/v0.20.0/wancode_0.20.0_x64-setup.exe") {
    throw "origin URL 错误：$($originPlatform.url)"
  }
  if ($mirrorPlatform.url -ne "https://gh-proxy.com/$($originPlatform.url)") {
    throw "镜像 URL 错误：$($mirrorPlatform.url)"
  }
  foreach ($path in $paths) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
      throw "$path 含 UTF-8 BOM"
    }
  }
  Write-Host "UPDATE MANIFEST CONTRACT PASS"
} finally {
  Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
