# 有效树规范化与构建清单共享函数库（#126 B1 复核整改：单一事实源）
# 被 audit_effective_tree.ps1 / migration_audit.ps1 dot-source。
# 规范化语义（设计稿 §3）：排除 .git/、target/、*.audit-tmp；
# 「相对路径<TAB>sha256」按 Ordinal 排序；清单 UTF-8+LF 整体 sha256 = effective_tree_sha256。

function Get-NormalizedManifest([string]$dir) {
  $dir = (Resolve-Path $dir).Path
  $lines = [System.Collections.Generic.List[string]]::new()
  Get-ChildItem -Path $dir -Recurse -File -Force | ForEach-Object {
    $rel = $_.FullName.Substring($dir.Length).TrimStart('\', '/') -replace '\\', '/'
    # A normal clone has a `.git/` directory, while `git worktree add` creates a
    # root `.git` pointer file. Both are Git metadata and must be excluded from
    # the effective product tree. Without the exact-file check, byte-identical
    # worktrees produce a different digest solely because of that pointer.
    if ($rel -eq '.git' -or $rel -like '.git/*' -or $rel -like 'target/*' -or $rel -like '*/target/*' -or $rel -like '*.audit-tmp') { return }
    $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
    $lines.Add("$rel`t$h")
  }
  $lines.Sort([System.StringComparer]::Ordinal)
  return $lines
}

function Get-ManifestDigest($lines) {
  $bytes = [System.Text.Encoding]::UTF8.GetBytes(($lines -join "`n") + "`n")
  $sha = [System.Security.Cryptography.SHA256]::Create()
  return ([System.BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').ToLowerInvariant()
}

function Get-FileSha([string]$path) {
  return (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
}

# 读取构建清单（六字段全格式，verify/审计用；旧格式 lock 请自行只取 repo/commit）
function Read-BuildManifest([string]$lockPath) {
  $m = @{}
  Get-Content $lockPath | Where-Object { $_ -match '^[a-z0-9_]+=' } | ForEach-Object {
    $k, $v = $_ -split '=', 2
    $m[$k] = $v
  }
  foreach ($k in "repo", "commit", "wiring_patch_sha256", "emergency_patch_sha256", "cargo_lock_sha256", "effective_tree_sha256") {
    if (-not $m[$k]) { throw "构建清单缺字段：$k（$lockPath）" }
  }
  return $m
}

# 两清单逐项比对；差异逐条打印，返回差异数
function Compare-NormalizedManifests($linesA, $linesB, [string]$labelA = "A", [string]$labelB = "B") {
  $mapA = @{}; foreach ($l in $linesA) { $p, $h = $l -split "`t"; $mapA[$p] = $h }
  $mapB = @{}; foreach ($l in $linesB) { $p, $h = $l -split "`t"; $mapB[$p] = $h }
  $bad = 0
  foreach ($p in $mapA.Keys) {
    if (-not $mapB.ContainsKey($p)) { Write-Host "只在 ${labelA}：$p"; $bad++ }
    elseif ($mapA[$p] -ne $mapB[$p]) { Write-Host "内容不同：$p"; $bad++ }
  }
  foreach ($p in $mapB.Keys) { if (-not $mapA.ContainsKey($p)) { Write-Host "只在 ${labelB}：$p"; $bad++ } }
  return $bad
}
