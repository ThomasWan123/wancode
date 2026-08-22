# WanCode 发布流水线（一条命令出签名安装包 + latest.json）
#
# 用法：  pwsh -File scripts/release.ps1 -Version 0.7.0
#
# 为什么签名是独立步骤而不是 build 时自动做：
#   updater 签名密钥是加密的（带密码，哪怕空密码）。tauri build 走到签名
#   时要解密密钥、读 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 环境变量；但
#   Windows/PowerShell 在 spawn 子进程时会丢弃**空字符串**环境变量
#   （子进程看到 undefined），于是 tauri 回退到交互式密码提示 → 后台
#   构建无 stdin → 卡死/跳过签名。用 `signer sign -f <key> --password=`
#   （空密码走 CLI 参数，不受此坑影响）在 build 后补签，稳定可靠。
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Repo = "ThomasWan123/wancode",
  # 国内直连 GitHub 资产 CDN（release-assets.githubusercontent.com）概率性失败，
  # 更新器下载走镜像前缀转发（原样转发，签名不变仍有效）。置 "" 可关。
  [string]$Mirror = "https://gh-proxy.com/",
  # 仅供本机 WiX/Windows Installer 服务不可用时验证 NSIS + updater 签名链。
  # 正式发布仍须省略此开关，由精确 SHA 的干净 Windows CI 同时产出 MSI/NSIS。
  [switch]$NsisOnly
)
$ErrorActionPreference = "Stop"

# PowerShell 5.1 明确拒绝（2026-07-30 实锤）：EAP=Stop 下 5.1 会把原生命令
# （tauri CLI/node）的普通 stderr 信息行包装成终止错误，build 一开口就死。
# pwsh 7 不包装。别删这个门闩——它替代的是"每次发版踩一遍再想起来"。
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw "release.ps1 需要 PowerShell 7+（pwsh）。当前 $($PSVersionTable.PSVersion)。安装：winget install Microsoft.PowerShell；或按脚本步骤在 bash 中分步执行（build→signer sign→latest.json）。"
}
$key = "$env:USERPROFILE\.tauri\wancode_updater.key"
$root = Split-Path $PSScriptRoot -Parent
# 引擎在仓库兄弟目录（见 vendor/grok-build.lock），产物落引擎 workspace 的 target
$bundle = Join-Path (Split-Path $root -Parent) "grok-build\target\release\bundle"

# 预检：只杀占用构建输出的 dev 实例（按可执行文件路径精确匹配）。
# 禁止 taskkill /IM wancode.exe —— 会误杀用户正在使用的安装版
# （%LOCALAPPDATA%\wancode\wancode.exe），教训见 2026-07-23 dogfood 日志。
$devExe = Join-Path (Split-Path $root -Parent) "grok-build\target\release\wancode.exe"
Get-Process wancode -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -eq $devExe } |
  ForEach-Object { Write-Host "停止 dev 实例 pid=$($_.Id)（$($_.Path)）"; Stop-Process -Id $_.Id -Force }

# 工具链环境（Windows 专用坑：lld-link 绕 PDB 上限 + 扩栈）
$env:Path = "$env:Path;$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.protoc\bin;C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin"
$env:PROTOC = "$env:USERPROFILE\.protoc\bin\protoc.exe"
$env:RUSTFLAGS = "-C link-arg=/STACK:16777216"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "lld-link"

Write-Host "[1/5] dev 构建输出占用检查完成（安装版不受影响）..."

Write-Host "[2/5] 取回并校验 PDFium 运行时（URL/归档/DLL 哈希均由 vendor/pdfium.lock 钉死）..."
& "$PSScriptRoot/fetch_pdfium.ps1"
if ($LASTEXITCODE -ne 0) { throw "PDFium 供应链取回/校验失败" }

Write-Host "[3/5] 构建 release（不在 build 时签名——见文件头注释）..."
Set-Location $root
if ($NsisOnly) {
  Write-Host "  NSIS-only 本机验证模式：跳过 WiX；不得据此声称 MSI 已验证。" -ForegroundColor Yellow
  npm run tauri build -- --bundles nsis
} else {
  npm run tauri build
}
if ($LASTEXITCODE -ne 0) { throw "tauri build 失败" }

# tauri build 内部调 cargo，无法直接传 --locked——改为构建后断言：
# 引擎工作树 Cargo.lock 未被 Cargo 改写（与 vendor 覆盖文件逐字节一致）。
$engineLock = Join-Path (Split-Path $root -Parent) "grok-build\Cargo.lock"
$vendorLock = Join-Path $root "vendor\grok-build-Cargo.lock"
if ((Get-FileHash $engineLock).Hash -ne (Get-FileHash $vendorLock).Hash) {
  throw "构建后引擎 Cargo.lock 与 vendor/grok-build-Cargo.lock 不一致（依赖漂移）。请从干净有效树再生覆盖文件后重新发布。"
}

$setup = "$bundle\nsis\wancode_${Version}_x64-setup.exe"
$msi = "$bundle\msi\wancode_${Version}_x64_en-US.msi"
if (-not (Test-Path $setup)) { throw "找不到 $setup（版本号对不上？）" }

Write-Host "[4/5] 补签 setup.exe（signer sign，空密码走 CLI 参数）..."
# 当前 Tauri CLI/Clap 接受 `--password=` 作为明确的空密码。不要传
# `-p '""'`：新版 CLI 会把两个引号字符当成真实密码并报 Wrong password。
# 只用 package-lock 已安装的 CLI；发布时禁止 npx 临时下载另一版本。
npx --no-install @tauri-apps/cli signer sign -f $key --password= $setup
if ($LASTEXITCODE -ne 0) { throw "签名失败" }
$sig = Get-Content "$setup.sig" -Raw

Write-Host "[5/5] 生成 origin + 镜像 manifests..."
$pub = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
. "$PSScriptRoot/update-manifests.ps1"
$null = Write-WanCodeUpdateManifests `
  -Bundle $bundle `
  -Version $Version `
  -Repo $Repo `
  -Mirror $Mirror `
  -Signature $sig `
  -PubDate $pub

Write-Host ""
Write-Host "✅ 完成。产物：" -ForegroundColor Green
if (-not $NsisOnly -and (Test-Path $msi)) { Write-Host "   $msi" }
Write-Host "   $setup"
Write-Host "   $setup.sig"
Write-Host "   $bundle\latest.json"
Write-Host "   $bundle\latest-gh-proxy.json"
Write-Host ""
Write-Host "下一步（手动，发布是外向操作）：" -ForegroundColor Yellow
if ($NsisOnly) {
  Write-Host "   NSIS-only 结果不得发布；先取得精确 SHA 的 CI MSI/NSIS，再从合并后 main 重建并签名。" -ForegroundColor Yellow
} else {
  Write-Host "   git tag v$Version; git push origin v$Version"
  Write-Host "   gh release create v$Version `"$msi`" `"$setup`" `"$setup.sig`" `"$bundle\latest.json`" `"$bundle\latest-gh-proxy.json`" --repo $Repo --title `"WanCode v$Version`" --notes `"...`""
}
Write-Host ""
Write-Host "发布后硬断言（v0.18.9 事故复盘：gh 的 file#label 语法只改显示标签不改资产文件名，"
Write-Host "曾把 latest.json 传成 latest-189.json——updater 按文件名取件，全体用户 404）："
Write-Host "   `$assets = gh release view v$Version --json assets --jq '.assets[].name'"
Write-Host "   if (`$assets -notcontains 'latest.json') { throw '资产名必须精确为 latest.json' }"
Write-Host "   if (`$assets -notcontains 'latest-gh-proxy.json') { throw '缺少镜像清单 latest-gh-proxy.json' }"

Write-Host ""
Write-Host "══════════ 发版强制检查单（v0.12.2 起，全过才发）══════════" -ForegroundColor Yellow
Write-Host "  [ ] 1. 真零配置首启 smoke：挪走 ~/.grok/config.toml 启动，应弹向导且 60 秒不崩"
Write-Host "  [ ] 2. 老配置升级 smoke：现有配置启动，会话可用"
Write-Host "  [ ] 3. Rust 单测全绿：cargo test --locked -j 1 -p wancode --lib（Windows 串行链接，避免同名 DLL 争用）"
Write-Host "  [ ] 4. 上传后双源验证：latest.json / latest-gh-proxy.json 同版本同签名，安装包首 KB 为 MZ 头"
Write-Host "  [ ] 5. 资产名断言：release 资产列表必须含 latest.json 与 latest-gh-proxy.json（禁用 file#label 改名上传）"
Write-Host "  （教训：v0.12.0 发布后才发现新用户装机即闪退——历史所有版本都没测过第 1 条）"
Write-Host "═══════════════════════════════════════════════════════════" -ForegroundColor Yellow
