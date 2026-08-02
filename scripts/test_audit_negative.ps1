# verify 负向门测试（#126 B1 复核定案 P1）：证明 fail-closed 门在错误状态下真的会红。
#
# 用临时夹具（微型引擎 git 仓库 + 夹具 -Root）驱动 audit_effective_tree.ps1 verify，
# 每个场景断言【非零退出码 + 错误原因文本命中】，不接受"反正失败了"。
# 场景：正向对照 / wiring 哈希错误 / none 但 emergency 非空 / emergency 已到期 /
#       emergency 缺事故编号或到期版本 / 合法 emergency 正向（非空+元数据齐备+未过期）/
#       有效树多一个文件(porcelain) / 改一个文件(摘要)。
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "effective_tree_lib.ps1")
$auditScript = Join-Path $PSScriptRoot "audit_effective_tree.ps1"

$fx = Join-Path ([System.IO.Path]::GetTempPath()) ("audit-negative-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
$fxRoot = Join-Path $fx "root"          # 夹具 wancode 根（vendor/ + src-tauri/tauri.conf.json）
$eng = Join-Path $fx "engine"           # 微型引擎仓库
New-Item -ItemType Directory -Path (Join-Path $fxRoot "vendor"), (Join-Path $fxRoot "src-tauri"), $eng | Out-Null
Set-Content -Path (Join-Path $fxRoot "src-tauri\tauri.conf.json") -Value '{"version":"0.18.9"}' -Encoding ascii

function W([string]$path, [string]$content) {
  # 统一 LF 原始字节写入（夹具与生产同语义：字节确定）
  [System.IO.File]::WriteAllText($path, $content.Replace("`r`n", "`n"), [System.Text.UTF8Encoding]::new($false))
}

# ── 微型引擎仓库：一个源文件 + Cargo.lock，提交为基线 ──
git -C $eng init -q
git -C $eng config core.autocrlf false
git -C $eng config user.email "fixture@test"; git -C $eng config user.name "fixture"
W (Join-Path $eng "a.txt") "hello`n"
W (Join-Path $eng "b.txt") "emergency target`n"   # 供合法 emergency 正向场景改动
W (Join-Path $eng "Cargo.lock") "orig-lock`n"
git -C $eng add -A; git -C $eng commit -q -m base
$engCommit = (git -C $eng rev-parse HEAD).Trim()

# wiring patch：用 git diff 真实生成（避免手写 index 行出错）
W (Join-Path $eng "a.txt") "hello patched`n"
$wiring = Join-Path $fxRoot "vendor\grok-build-wiring.patch"
cmd /c "git -C `"$eng`" diff > `"$wiring`"" | Out-Null
git -C $eng checkout -q -- a.txt

# 应用补丁 + 覆盖（生产同序），emergency 常态 0 字节
git -C $eng apply $wiring
$overlay = Join-Path $fxRoot "vendor\grok-build-Cargo.lock"
W $overlay "overlay-lock`n"
Copy-Item $overlay (Join-Path $eng "Cargo.lock") -Force
$emerg = Join-Path $fxRoot "vendor\grok-build-emergency.patch"
[System.IO.File]::WriteAllBytes($emerg, @())

# 构建清单（哈希按夹具实际值计算）
$digest = Get-ManifestDigest (Get-NormalizedManifest $eng)
$lock = Join-Path $fxRoot "vendor\grok-build.lock"
function Write-FixtureLock([string]$wiringSha, [string]$emergSha, [string]$treeSha) {
  W $lock ("repo=unused`ncommit=$engCommit`nwiring_patch_sha256=$wiringSha`nemergency_patch_sha256=$emergSha`ncargo_lock_sha256=$(Get-FileSha $overlay)`neffective_tree_sha256=$treeSha`n")
}
Write-FixtureLock (Get-FileSha $wiring) "none" $digest

# ── 场景执行器：跑子进程 verify，断言退出码与错误文本 ──
$script:pass = 0; $script:fail = 0
function Assert-Verify([string]$name, [int]$expectZero, [string]$pattern) {
  $out = (& powershell -NoProfile -File $auditScript verify -Engine $eng -Root $fxRoot 2>&1 | Out-String)
  $code = $LASTEXITCODE
  $codeOk = if ($expectZero) { $code -eq 0 } else { $code -ne 0 }
  $msgOk = ($out -match [regex]::Escape($pattern))
  if ($codeOk -and $msgOk) {
    Write-Host ("PASS {0}（exit={1}，命中：{2}）" -f $name, $code, $pattern)
    $script:pass++
  } else {
    Write-Host ("FAIL {0}：exit={1}（期望 {2}），输出未命中 '{3}'：`n{4}" -f $name, $code, $(if ($expectZero) { "0" } else { "非零" }), $pattern, $out) -ForegroundColor Red
    $script:fail++
  }
}

# 0) 正向对照：夹具本身必须绿（否则后续红全是假阴性）
Assert-Verify "正向对照" 1 "VERIFY OK"

# 1) wiring 哈希错误
Write-FixtureLock ("0" * 64) "none" $digest
Assert-Verify "wiring哈希错误" 0 "wiring patch 哈希不符"
Write-FixtureLock (Get-FileSha $wiring) "none" $digest

# 2) 清单 none 但 emergency 非空
W $emerg "# stray`n"
Assert-Verify "none但emergency非空" 0 "声明 none 但 emergency patch 非 0 字节"
[System.IO.File]::WriteAllBytes($emerg, @())

# 3) emergency 已到期（当前 0.18.9 ≥ 到期 0.0.1）
W $emerg "# incident: INC-TEST-1`n# expires_in_version: 0.0.1`n"
Write-FixtureLock (Get-FileSha $wiring) (Get-FileSha $emerg) $digest
Assert-Verify "emergency已到期" 0 "已到期"

# 4) 非空 emergency 缺事故编号/到期版本
W $emerg "# no metadata here`n"
Write-FixtureLock (Get-FileSha $wiring) (Get-FileSha $emerg) $digest
Assert-Verify "emergency缺头部元数据" 0 "缺事故编号或到期版本"
[System.IO.File]::WriteAllBytes($emerg, @())
Write-FixtureLock (Get-FileSha $wiring) "none" $digest

# 5) 合法 emergency patch（非空 + 元数据齐备 + 未过期）必须【绿】。
#    只有负向场景时，emergency 整条分支即便全盘失效也不会被发现——本场景
#    是唯一能证明"启用紧急补丁后 verify 仍可通过"的证据。
$emergDiff = Join-Path $fx "emergency.diff"
W (Join-Path $eng "b.txt") "emergency patched`n"
cmd /c "git -C `"$eng`" diff -- b.txt > `"$emergDiff`"" | Out-Null
git -C $eng checkout -q -- b.txt
# 元数据须落在前 10 行内（verify 只读文件头）；git apply 会跳过 diff 头之前的前言。
W $emerg ("# incident: INC-TEST-OK`n# expires_in_version: 99.0.0`n" + [System.IO.File]::ReadAllText($emergDiff))
git -C $eng apply $emerg
if ($LASTEXITCODE -ne 0) { throw "夹具自身有问题：合法 emergency patch 应用失败" }
Write-FixtureLock (Get-FileSha $wiring) (Get-FileSha $emerg) (Get-ManifestDigest (Get-NormalizedManifest $eng))
Assert-Verify "合法emergency正向" 1 "VERIFY OK"
# 还原到 emergency=none 基线（b.txt 复原 + 清空补丁 + 清单摘要回退）
git -C $eng checkout -q -- b.txt
[System.IO.File]::WriteAllBytes($emerg, @())
Write-FixtureLock (Get-FileSha $wiring) "none" $digest

# 6) 有效树多一个文件 → porcelain 精确集合红
W (Join-Path $eng "stray.txt") "extra`n"
Assert-Verify "有效树多一个文件" 0 "porcelain 集合与 patch 触及"
Remove-Item (Join-Path $eng "stray.txt")

# 7) 改一个 patch 触及文件的内容 → porcelain 集合不变，摘要复算红
W (Join-Path $eng "a.txt") "hello tampered`n"
Assert-Verify "有效树改一个文件" 0 "effective_tree_sha256 复算"
git -C $eng checkout -q -- a.txt; git -C $eng apply $wiring   # 还原

# 收尾：正向再对照一次，证明还原干净（场景间无串扰）
Assert-Verify "还原后正向对照" 1 "VERIFY OK"

Remove-Item -Recurse -Force $fx -ErrorAction SilentlyContinue
Write-Host ""
Write-Host ("负向门测试：{0} pass / {1} fail" -f $script:pass, $script:fail)
if ($script:fail -gt 0) { exit 1 }
Write-Host "NEGATIVE GATE OK：全部场景按预期红/绿且错误原因命中"
