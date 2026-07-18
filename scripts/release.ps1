# WanCode 发布流水线（一条命令出签名安装包 + latest.json）
#
# 用法：  pwsh -File scripts/release.ps1 -Version 0.7.0
#
# 为什么签名是独立步骤而不是 build 时自动做：
#   updater 签名密钥是加密的（带密码，哪怕空密码）。tauri build 走到签名
#   时要解密密钥、读 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 环境变量；但
#   Windows/PowerShell 在 spawn 子进程时会丢弃**空字符串**环境变量
#   （子进程看到 undefined），于是 tauri 回退到交互式密码提示 → 后台
#   构建无 stdin → 卡死/跳过签名。用 `signer sign -f <key> -p ""`
#   （空密码走 CLI 参数，不受此坑影响）在 build 后补签，稳定可靠。
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Repo = "ThomasWan123/wancode",
  # 国内直连 GitHub 资产 CDN（release-assets.githubusercontent.com）概率性失败，
  # 更新器下载走镜像前缀转发（原样转发，签名不变仍有效）。置 "" 可关。
  [string]$Mirror = "https://gh-proxy.com/"
)
$ErrorActionPreference = "Stop"
$key = "$env:USERPROFILE\.tauri\wancode_updater.key"
$root = Split-Path $PSScriptRoot -Parent
$bundle = "D:\WANCode\grok-build\target\release\bundle"

# 工具链环境（Windows 专用坑：lld-link 绕 PDB 上限 + 扩栈）
$env:Path = "$env:Path;$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.protoc\bin;C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin"
$env:PROTOC = "$env:USERPROFILE\.protoc\bin\protoc.exe"
$env:RUSTFLAGS = "-C link-arg=/STACK:16777216"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "lld-link"

Write-Host "[1/4] 关闭运行中的 wancode（否则 exe 被占用无法覆盖）..."
Get-Process wancode -EA SilentlyContinue | Stop-Process -Force -Confirm:$false

Write-Host "[2/4] 构建 release（不在 build 时签名——见文件头注释）..."
Set-Location $root
npm run tauri build
if ($LASTEXITCODE -ne 0) { throw "tauri build 失败" }

$setup = "$bundle\nsis\wancode_${Version}_x64-setup.exe"
$msi = "$bundle\msi\wancode_${Version}_x64_en-US.msi"
if (-not (Test-Path $setup)) { throw "找不到 $setup（版本号对不上？）" }

Write-Host "[3/4] 补签 setup.exe（signer sign，空密码走 CLI 参数）..."
npx --yes @tauri-apps/cli signer sign -f $key -p "" $setup
if ($LASTEXITCODE -ne 0) { throw "签名失败" }
$sig = Get-Content "$setup.sig" -Raw

Write-Host "[4/4] 生成 latest.json..."
$pub = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$latest = @{
  version   = $Version
  notes     = "WanCode v$Version"
  pub_date  = $pub
  platforms = @{
    "windows-x86_64" = @{
      signature = $sig.Trim()
      url       = "$Mirror`https://github.com/$Repo/releases/download/v$Version/wancode_${Version}_x64-setup.exe"
    }
  }
} | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText("$bundle\latest.json", $latest, (New-Object System.Text.UTF8Encoding($false)))

Write-Host ""
Write-Host "✅ 完成。产物：" -ForegroundColor Green
Write-Host "   $msi"
Write-Host "   $setup"
Write-Host "   $setup.sig"
Write-Host "   $bundle\latest.json"
Write-Host ""
Write-Host "下一步（手动，发布是外向操作）：" -ForegroundColor Yellow
Write-Host "   git tag v$Version; git push origin v$Version"
Write-Host "   gh release create v$Version `"$msi`" `"$setup`" `"$setup.sig`" `"$bundle\latest.json`" --repo $Repo --title `"WanCode v$Version`" --notes `"...`""
