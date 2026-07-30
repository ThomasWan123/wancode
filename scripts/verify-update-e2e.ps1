# verify-update-e2e.ps1
#
# Fully automated, isolated end-to-end verification of the wancode auto-update
# chain 0.18.5 -> 0.18.6 (task #123).
#
# What it does (all unattended):
#   1. Safety preflight: refuses to run if any wancode.exe is running outside the
#      isolated directory (the tauri NSIS installer kills wancode.exe BY NAME in
#      silent/passive mode), snapshots the real install's registry keys, exe hash
#      and shortcut targets.
#   2. Fetches latest.json from the updater endpoint, asserts version/URL.
#   3. Downloads the 0.18.6 installer named by latest.json and verifies its
#      minisign signature (real Ed25519 + BLAKE2b-512 prehash, pure-python
#      implementation, pubkey taken from src-tauri/tauri.conf.json).
#   4. Downloads the 0.18.5 installer (+ .sig, also verified) and installs it
#      into an isolated directory using /S /NS /D=<dir>.
#   5. Replays the updater step exactly like tauri-plugin-updater 2.10.1 does on
#      Windows: launches the 0.18.6 installer with /P /R /UPDATE (plus /D=<dir>
#      to force the isolated target), waits for exit 0, asserts the isolated exe
#      is now 0.18.6, asserts /R relaunched the app, then kills ONLY processes
#      running from the isolated directory.
#   6. Restores the registry snapshot and audits that the real installation
#      (%LOCALAPPDATA%\wancode, HKCU uninstall key = 0.18.6, shortcuts) is
#      byte-for-byte untouched.
#
# The real installation is never written to. Registry keys are polluted by the
# isolated installs by design (the NSIS template writes them unconditionally)
# and are restored from a snapshot before the script exits, even on failure.
#
# Run:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-update-e2e.ps1
#
# Idempotent: safe to re-run; caches downloads, recreates the isolated install.

[CmdletBinding()]
param(
    [string]$WorkDir = (Join-Path $env:TEMP 'wancode-e2e'),
    [int]$InstallTimeoutSec = 300,
    [switch]$KeepIsolatedInstall
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# ---------------------------------------------------------------- constants --
$RepoSlug     = 'ThomasWan123/wancode'
# 版本对可传参：发布门每次验"上一版 -> 本版"。无参默认最近一对。
$OldVersion   = if ($env:WUE_OLD) { $env:WUE_OLD } else { '0.18.5' }
$NewVersion   = if ($env:WUE_NEW) { $env:WUE_NEW } else { '0.18.6' }
$GhProxy      = 'https://gh-proxy.com/'
$RealDir      = Join-Path $env:LOCALAPPDATA 'wancode'
$RealExe      = Join-Path $RealDir 'wancode.exe'
$UninstKey    = 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\wancode'
$ManuKey      = 'HKCU\Software\wanwe\wancode'   # NSIS MANUPRODUCTKEY (Software\<publisher>\<product>)
$IsoDir       = Join-Path $WorkDir 'app'        # isolated install target; MUST contain no spaces (/D=)
$RepoRoot     = Split-Path -Parent $PSScriptRoot
$TauriConf    = Join-Path $RepoRoot 'src-tauri\tauri.conf.json'

if ($IsoDir -match ' ') { throw "IsoDir '$IsoDir' contains a space; NSIS /D= cannot be quoted. Pick a space-free WorkDir." }
New-Item -ItemType Directory -Force $WorkDir | Out-Null

# ---------------------------------------------------------------- reporting --
$script:Results     = New-Object System.Collections.ArrayList
$script:AnyFail     = $false
$script:RegPolluted = $false
$script:RegRestored = $false

function Step {
    param([string]$Name, [bool]$Ok, [string]$Detail = '')
    if (-not $Ok) { $script:AnyFail = $true }
    $tag  = if ($Ok) { 'PASS' } else { 'FAIL' }
    $line = ('[{0}] {1}' -f $tag, $Name)
    if ($Detail) { $line += (' -- ' + $Detail) }
    Write-Host $line
    [void]$script:Results.Add($line)
}
function Info { param([string]$Msg) Write-Host ("       " + $Msg) }

# ------------------------------------------------------------------ helpers --
function Test-DownloadValid {
    param([string]$Path, [bool]$ExpectPE)
    if (-not (Test-Path $Path)) { return $false }
    $len = (Get-Item $Path).Length
    if ($len -le 0) { return $false }
    if ($ExpectPE) {
        if ($len -lt 1MB) { return $false }
        $fs = [IO.File]::OpenRead($Path)
        try { $b0 = $fs.ReadByte(); $b1 = $fs.ReadByte() } finally { $fs.Close() }
        return ($b0 -eq 0x4D -and $b1 -eq 0x5A)   # 'MZ'
    }
    return $true
}

function Download-File {
    param([string]$Url, [string]$OutFile, [bool]$ExpectPE = $false, [switch]$ReuseCached)
    if ($ReuseCached -and (Test-DownloadValid $OutFile $ExpectPE)) { return '<cached>' }
    # candidate mirrors: as given, gh-proxy wrapped, and (if already wrapped) the direct github URL
    $candidates = @($Url)
    if ($Url.StartsWith($GhProxy)) { $candidates += $Url.Substring($GhProxy.Length) }
    else                           { $candidates += ($GhProxy + $Url) }
    $tried = @()
    foreach ($attempt in 1..2) {
        foreach ($u in $candidates) {
            $tried += $u
            if (Test-Path $OutFile) { Remove-Item -Force $OutFile }
            try {
                Invoke-WebRequest -Uri $u -OutFile $OutFile -UseBasicParsing -TimeoutSec 180
                if (Test-DownloadValid $OutFile $ExpectPE) { return $u }
            } catch { }
        }
        Start-Sleep -Seconds 3
    }
    throw ("download failed or produced invalid content, tried: " + (($tried | Select-Object -Unique) -join ' ; '))
}

function Get-ExeVersion {
    param([string]$Path)
    $vi = (Get-Item $Path).VersionInfo
    $v = $vi.FileVersion
    if (-not $v) { $v = $vi.ProductVersion }
    if ($v) { return $v.Trim() } else { return '' }
}

function Get-WancodeProcs {
    Get-Process wancode -ErrorAction SilentlyContinue | Where-Object { $_.Path }
}
function Get-ForeignWancodeProcs {
    Get-WancodeProcs | Where-Object { $_.Path -notlike ($IsoDir + '\*') }
}
function Get-IsoWancodeProcs {
    Get-WancodeProcs | Where-Object { $_.Path -like ($IsoDir + '\*') }
}

function Get-LnkTarget {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return '<absent>' }
    $sh = New-Object -ComObject WScript.Shell
    try { return $sh.CreateShortcut($Path).TargetPath } finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($sh) }
}

function Get-RegText {
    param([string]$Key)
    $out = cmd /c "reg query `"$Key`" /s 2>nul"
    if ($LASTEXITCODE -ne 0) { return '<absent>' }
    return (($out | Where-Object { $_ -ne '' }) -join "`n")
}

function Run-Installer {
    param([string]$Exe, [string]$Arguments)
    $p = Start-Process -FilePath $Exe -ArgumentList $Arguments -PassThru
    if (-not $p.WaitForExit($InstallTimeoutSec * 1000)) {
        try { $p.Kill() } catch { }
        return @{ Ok = $false; Code = -999; Detail = "timeout after $InstallTimeoutSec s" }
    }
    Start-Sleep -Milliseconds 300
    return @{ Ok = ($p.ExitCode -eq 0); Code = $p.ExitCode; Detail = ("exit=" + $p.ExitCode) }
}

function Restore-Registry {
    if (-not $script:RegPolluted -or $script:RegRestored) { return }
    cmd /c "reg delete `"$UninstKey`" /f 2>nul" | Out-Null
    cmd /c "reg delete `"$ManuKey`" /f 2>nul"   | Out-Null
    cmd /c "reg import `"$script:SnapUninst`" 2>nul" | Out-Null
    $r1 = $LASTEXITCODE
    cmd /c "reg import `"$script:SnapManu`" 2>nul"   | Out-Null
    $r2 = $LASTEXITCODE
    $script:RegRestored = $true
    Step 'Registry restore (reg import of pre-run snapshot)' (($r1 -eq 0) -and ($r2 -eq 0)) ("import exit codes: $r1,$r2")
}

function Final-Audit {
    # Runs unconditionally before exit: prove the real install was never touched.
    $hashNow = (Get-FileHash -Algorithm SHA256 $RealExe).Hash
    Step 'Real install exe untouched (sha256 identical)' ($hashNow -eq $script:RealExeHash) ("sha256=" + $hashNow.Substring(0,16) + '...')

    $uninstNow = Get-RegText $UninstKey
    Step 'Real uninstall registry key restored bit-identical' ($uninstNow -eq $script:PreUninstText) ''
    $manuNow = Get-RegText $ManuKey
    Step 'Real Software\wanwe\wancode key restored bit-identical' ($manuNow -eq $script:PreManuText) ''

    $dv = (Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\wancode' -ErrorAction SilentlyContinue).DisplayVersion
    # 断言的是"与跑前一致"，不是等于 $NewVersion——本 E2E 从不升级真实安装，
    # 真实安装的版本是什么就该保持什么。旧断言硬编码 NewVersion，首轮里真实
    # 安装恰好等于 NewVersion（0.18.6），巧合掩盖了错误；0.18.7 一发布立刻假红。
    Step ("Real install DisplayVersion unchanged (" + $script:PreRealDisplayVersion + ")") ($dv -eq $script:PreRealDisplayVersion) ("DisplayVersion=" + $dv)

    $dt = Get-LnkTarget $script:DesktopLnk
    $st = Get-LnkTarget $script:StartMenuLnk
    Step 'Desktop shortcut target unchanged' ($dt -eq $script:PreDesktopTarget) ("target=" + $dt)
    Step 'Start-menu shortcut target unchanged' ($st -eq $script:PreStartTarget) ("target=" + $st)
}

function Finish {
    param([int]$Code)
    Restore-Registry
    Final-Audit
    # kill any leftover isolated processes; never touch anything else
    Get-IsoWancodeProcs | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch { } }
    if (-not $KeepIsolatedInstall) {
        try { Remove-Item -Recurse -Force $IsoDir -ErrorAction Stop } catch { Info "note: could not remove $IsoDir (file lock?); harmless." }
    }
    Write-Host ''
    Write-Host '==================== SUMMARY ===================='
    $script:Results | ForEach-Object { Write-Host $_ }
    $overall = if ($script:AnyFail) { 'OVERALL: FAIL' } else { 'OVERALL: PASS' }
    Write-Host $overall
    if ($script:AnyFail) { exit 1 } else { exit $Code }
}

# ------------------------------------------------- embedded minisign verify --
# Pure-python minisign verification (Ed25519 per RFC 8032 + BLAKE2b-512 prehash,
# stdlib only). Verifies both the file signature and the trusted-comment
# global signature, and that the signature key id matches the configured pubkey.
$MinisignPy = @'
import sys, base64, hashlib

p = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
d = (-121665 * pow(121666, p - 2, p)) % p
I = pow(2, (p - 1) // 4, p)

def recover_x(y, sign):
    if y >= p: return None
    x2 = (y*y - 1) * pow(d*y*y + 1, p - 2, p) % p
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (p + 3) // 8, p)
    if (x*x - x2) % p != 0:
        x = x * I % p
    if (x*x - x2) % p != 0:
        return None
    if (x & 1) != sign:
        x = p - x
    return x

def pt_add(P, Q):
    A = (P[1]-P[0]) * (Q[1]-Q[0]) % p
    B = (P[1]+P[0]) * (Q[1]+Q[0]) % p
    C = 2 * P[3] * Q[3] * d % p
    D = 2 * P[2] * Q[2] % p
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E*F % p, G*H % p, F*G % p, E*H % p)

def pt_mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1: Q = pt_add(Q, P)
        P = pt_add(P, P)
        s >>= 1
    return Q

def pt_eq(P, Q):
    return (P[0]*Q[2] - Q[0]*P[2]) % p == 0 and (P[1]*Q[2] - Q[1]*P[2]) % p == 0

gy = 4 * pow(5, p - 2, p) % p
gx = recover_x(gy, 0)
G = (gx, gy, 1, gx * gy % p)

def decompress(s):
    if len(s) != 32: return None
    y = int.from_bytes(s, 'little')
    sign = y >> 255
    y &= (1 << 255) - 1
    x = recover_x(y, sign)
    if x is None: return None
    return (x, y, 1, x * y % p)

def ed25519_verify(pk, msg, sig):
    A = decompress(pk)
    if A is None: return False
    R = decompress(sig[:32])
    ss = int.from_bytes(sig[32:], 'little')
    if R is None or ss >= L: return False
    h = int.from_bytes(hashlib.sha512(sig[:32] + pk + msg).digest(), 'little') % L
    return pt_eq(pt_mul(ss, G), pt_add(R, pt_mul(h, A)))

def b64_block(text):
    # Return (comment_lines, dict of parsed base64 lines) for a minisign text box
    return [l for l in text.replace('\r', '').split('\n') if l != '']

def main():
    pub_b64, sig_arg, file_path = sys.argv[1], sys.argv[2], sys.argv[3]

    pub_lines = b64_block(base64.b64decode(pub_b64).decode('utf-8'))
    pub_raw = base64.b64decode(pub_lines[1])
    if pub_raw[:2] != b'Ed' or len(pub_raw) != 42:
        print('minisign: bad public key structure'); sys.exit(2)
    key_id, pk = pub_raw[2:10], pub_raw[10:42]

    sig_text = open(sig_arg, 'rb').read().decode('utf-8').strip()
    if not sig_text.startswith('untrusted comment:'):
        sig_text = base64.b64decode(sig_text).decode('utf-8')
    lines = b64_block(sig_text)
    if len(lines) < 4:
        print('minisign: bad signature box'); sys.exit(2)
    sig_raw = base64.b64decode(lines[1])
    alg, sig_kid, sig64 = sig_raw[:2], sig_raw[2:10], sig_raw[10:74]
    trusted_comment = lines[2].split('trusted comment: ', 1)[1]
    global_sig = base64.b64decode(lines[3])

    if sig_kid != key_id:
        print('minisign: KEY ID MISMATCH (sig %s vs pub %s)' % (sig_kid.hex(), key_id.hex())); sys.exit(3)

    data = open(file_path, 'rb').read()
    if alg == b'ED':
        msg = hashlib.blake2b(data, digest_size=64).digest()
    elif alg == b'Ed':
        msg = data
    else:
        print('minisign: unknown algorithm %r' % alg); sys.exit(2)

    if not ed25519_verify(pk, msg, sig64):
        print('minisign: FILE SIGNATURE INVALID'); sys.exit(4)
    if not ed25519_verify(pk, sig64 + trusted_comment.encode('utf-8'), global_sig):
        print('minisign: TRUSTED COMMENT SIGNATURE INVALID'); sys.exit(5)

    print('minisign: OK alg=%s keyid=%s comment=%s' % (alg.decode(), key_id.hex(), trusted_comment))
    sys.exit(0)

main()
'@
$MinisignPyPath = Join-Path $WorkDir 'minisign_verify.py'
Set-Content -Path $MinisignPyPath -Value $MinisignPy -Encoding ASCII

$PythonExe = $null
$pyCmd = Get-Command python -ErrorAction SilentlyContinue
if ($pyCmd) { $PythonExe = $pyCmd.Source }

function Verify-Minisign {
    param([string]$Name, [string]$PubB64, [string]$SigFile, [string]$File)
    if (-not $PythonExe) {
        # Fail closed (Codex review): "sha256 computed but compared to nothing
        # + .sig merely exists" is not verification, and returning $true here
        # let an installer run UNVERIFIED. No python -> no signature check ->
        # no install. Period.
        Step ("$Name minisign signature") $false 'python unavailable - refusing to verify-by-existence; install blocked'
        return $false
    }
    $out = cmd /c "`"$PythonExe`" `"$MinisignPyPath`" `"$PubB64`" `"$SigFile`" `"$File`" 2>&1"
    $ok = ($LASTEXITCODE -eq 0)
    Step ("$Name minisign signature (Ed25519 + BLAKE2b prehash, local verification)") $ok (($out | Out-String).Trim())
    return $ok
}

# ============================================================== main =========
# Everything below runs inside try/catch so that ANY unhandled error still goes
# through Finish (registry restore + real-install audit) instead of leaving the
# machine polluted.
try {

# ============================================================== STEP 0: config
Write-Host "=== wancode $OldVersion -> $NewVersion auto-update E2E verification (isolated) ==="
Write-Host ("WorkDir  : " + $WorkDir)
Write-Host ("IsoDir   : " + $IsoDir)
Write-Host ("RealDir  : " + $RealDir)
Write-Host ''

$conf = Get-Content $TauriConf -Raw | ConvertFrom-Json
$PubKey   = $conf.plugins.updater.pubkey
$Endpoint = $conf.plugins.updater.endpoints[0]
Step 'Read updater config from tauri.conf.json' (($PubKey.Length -gt 0) -and ($Endpoint -like 'https://*')) ("endpoint=" + $Endpoint)

# ========================================================= STEP 1: preflight
if (-not (Test-Path $RealExe)) { Step 'Preflight: real install present' $false "missing $RealExe"; Finish 1 }
$script:RealExeHash = (Get-FileHash -Algorithm SHA256 $RealExe).Hash
$realVer = Get-ExeVersion $RealExe
Step 'Preflight: real install present' $true ("$RealExe version=$realVer sha256=" + $script:RealExeHash.Substring(0,16) + '...')

$foreign = @(Get-ForeignWancodeProcs)
if ($foreign.Count -gt 0) {
    Step 'Preflight: no wancode.exe running outside isolated dir' $false `
        ("REFUSING TO CONTINUE: the NSIS installer kills wancode.exe BY NAME in silent/passive mode. Running: " + (($foreign | ForEach-Object { $_.Path }) -join '; '))
    Finish 1
}
Step 'Preflight: no wancode.exe running outside isolated dir' $true ''

# registry + shortcut snapshot
$script:SnapUninst = Join-Path $WorkDir 'snapshot-uninstall.reg'
$script:SnapManu   = Join-Path $WorkDir 'snapshot-manu.reg'
cmd /c "reg export `"$UninstKey`" `"$script:SnapUninst`" /y" | Out-Null
$e1 = $LASTEXITCODE
cmd /c "reg export `"$ManuKey`" `"$script:SnapManu`" /y" | Out-Null
$e2 = $LASTEXITCODE
$script:PreUninstText = Get-RegText $UninstKey
$script:PreManuText   = Get-RegText $ManuKey
Step 'Preflight: registry snapshot (Uninstall\wancode + Software\wanwe\wancode)' (($e1 -eq 0) -and ($e2 -eq 0)) ("export exit codes: $e1,$e2")
$script:PreRealDisplayVersion = (Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\wancode' -EA SilentlyContinue).DisplayVersion
if (($e1 -ne 0) -or ($e2 -ne 0)) {
    # Fail closed (Codex review): the installer WILL rewrite these keys; if the
    # snapshot doesn't exist there is nothing to restore from afterwards.
    # Running on would trade a skipped test for corrupted real-install state.
    throw "registry snapshot failed (export codes $e1,$e2) - aborting BEFORE any installer runs"
}

$script:DesktopLnk   = Join-Path ([Environment]::GetFolderPath('Desktop')) 'wancode.lnk'
$script:StartMenuLnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\wancode\wancode.lnk'
if (-not (Test-Path $script:StartMenuLnk)) {
    $script:StartMenuLnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\wancode.lnk'
}
$script:PreDesktopTarget = Get-LnkTarget $script:DesktopLnk
$script:PreStartTarget   = Get-LnkTarget $script:StartMenuLnk
Info ("desktop lnk   : " + $script:DesktopLnk + ' -> ' + $script:PreDesktopTarget)
Info ("startmenu lnk : " + $script:StartMenuLnk + ' -> ' + $script:PreStartTarget)

# clean isolated dir from previous runs
Get-IsoWancodeProcs | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch { } }
if (Test-Path $IsoDir) {
    try { Remove-Item -Recurse -Force $IsoDir -ErrorAction Stop } catch {
        $IsoDir = Join-Path $WorkDir ('app-' + (Get-Date -Format 'HHmmss'))
        Info ("previous isolated dir locked; using fresh dir " + $IsoDir)
    }
}

# ====================================================== STEP 2: latest.json
$latestPath = Join-Path $WorkDir 'latest.json'
try {
    $src = Download-File -Url $Endpoint -OutFile $latestPath
    Step 'Fetch latest.json from updater endpoint' $true ("via " + $src)
} catch {
    Step 'Fetch latest.json from updater endpoint' $false $_.Exception.Message
    Finish 1
}
$latest = Get-Content $latestPath -Raw | ConvertFrom-Json
$plat = $latest.platforms.'windows-x86_64'
Step ("latest.json version is " + $NewVersion) ($latest.version -eq $NewVersion) ("version=" + $latest.version)
Step 'latest.json has windows-x86_64 url + signature' (($plat.url -like 'https://*') -and ($plat.signature.Length -gt 100)) ("url=" + $plat.url)

# ============================================ STEP 3: download + verify 0.18.6
$newExe = Join-Path $WorkDir ('wancode_' + $NewVersion + '_x64-setup.exe')
try {
    $src = Download-File -Url $plat.url -OutFile $newExe -ExpectPE $true -ReuseCached
    Step ("Download " + $NewVersion + " installer (updater URL)") $true ("via $src size=" + (Get-Item $newExe).Length)
} catch {
    Step ("Download " + $NewVersion + " installer (updater URL)") $false $_.Exception.Message
    Finish 1
}
$newSigFile = $newExe + '.sig'
Set-Content -Path $newSigFile -Value $plat.signature -Encoding ASCII -NoNewline
if (-not (Verify-Minisign -Name ($NewVersion + ' installer') -PubB64 $PubKey -SigFile $newSigFile -File $newExe)) {
    Info 'refusing to execute an installer that failed signature verification'
    Finish 1
}

# ============================================ STEP 4: download + verify 0.18.5
$oldUrl = "https://github.com/$RepoSlug/releases/download/v$OldVersion/wancode_${OldVersion}_x64-setup.exe"
$oldExe = Join-Path $WorkDir ('wancode_' + $OldVersion + '_x64-setup.exe')
try {
    $src = Download-File -Url $oldUrl -OutFile $oldExe -ExpectPE $true -ReuseCached
    Step ("Download " + $OldVersion + " installer") $true ("via $src size=" + (Get-Item $oldExe).Length)
} catch {
    Step ("Download " + $OldVersion + " installer") $false $_.Exception.Message
    Finish 1
}
$oldSigFile = $oldExe + '.sig'
$oldSigOk = $false
try {
    Download-File -Url ($oldUrl + '.sig') -OutFile $oldSigFile | Out-Null
    $oldSigOk = Verify-Minisign -Name ($OldVersion + ' installer') -PubB64 $PubKey -SigFile $oldSigFile -File $oldExe
} catch {
    Step ($OldVersion + ' installer minisign signature') $false ('.sig download failed: ' + $_.Exception.Message)
}
if (-not $oldSigOk) {
    Info 'refusing to execute an installer that failed signature verification'
    Finish 1
}

# ================================== STEP 5: install 0.18.5 into isolated dir
# From here on the registry is polluted by design; snapshot restore is armed.
$script:RegPolluted = $true

$r = Run-Installer -Exe $oldExe -Arguments ("/S /NS /D=" + $IsoDir)
Step ("Silent install " + $OldVersion + " to isolated dir (/S /NS /D=...)") $r.Ok $r.Detail
if (-not $r.Ok) { Finish 1 }

$isoExe = Join-Path $IsoDir 'wancode.exe'
$v = ''
if (Test-Path $isoExe) { $v = Get-ExeVersion $isoExe }
Step ("Isolated exe exists and is version " + $OldVersion) ($v -like ($OldVersion + '*')) ("version=" + $v)
if (-not (Test-Path $isoExe)) { Finish 1 }

# evidence: /D was respected and registry was (expectedly) redirected
$manuNow = (Get-ItemProperty -Path 'HKCU:\Software\wanwe\wancode' -ErrorAction SilentlyContinue).'(default)'
if (-not $manuNow) { $manuNow = (Get-Item 'HKCU:\Software\wanwe\wancode' -ErrorAction SilentlyContinue).GetValue('') }
$dvNow = (Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\wancode' -ErrorAction SilentlyContinue).DisplayVersion
Step '/D respected: install landed in isolated dir, not %LOCALAPPDATA%\wancode' ((Test-Path $isoExe) -and ($manuNow -eq $IsoDir)) ("MANUPRODUCTKEY=" + $manuNow)
Info ("expected registry pollution observed: Uninstall\wancode DisplayVersion=" + $dvNow + " (will be restored)")

$realHashMid = (Get-FileHash -Algorithm SHA256 $RealExe).Hash
Step 'Real install exe untouched after 0.18.5 isolated install' ($realHashMid -eq $script:RealExeHash) ''

# ==================== STEP 6: replay plugin update step (/P /R /UPDATE + /D)
# tauri-plugin-updater 2.10.1 on Windows launches the NSIS installer via
# ShellExecuteW with arguments "/P /R /UPDATE" and then exits. We replay the
# exact same arguments, adding only /D=<isolated dir> to redirect the target
# (verified above that /D wins over the registry-remembered location).
$foreign = @(Get-ForeignWancodeProcs)
if ($foreign.Count -gt 0) {
    Step 'Pre-update guard: no foreign wancode.exe running' $false ("aborting update replay; running: " + (($foreign | ForEach-Object { $_.Path }) -join '; '))
    Finish 1
}

$r = Run-Installer -Exe $newExe -Arguments ("/P /R /UPDATE /D=" + $IsoDir)
Step ("Update replay: run " + $NewVersion + " installer with plugin args /P /R /UPDATE (+/D)") $r.Ok $r.Detail
if (-not $r.Ok) { Finish 1 }

$v = Get-ExeVersion $isoExe
Step ("Isolated exe upgraded to " + $NewVersion) ($v -like ($NewVersion + '*')) ("version=" + $v)

# /R must have relaunched the (isolated) app
$relaunched = $null
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
    $relaunched = @(Get-IsoWancodeProcs)
    if ($relaunched.Count -gt 0) { break }
    Start-Sleep -Milliseconds 250
}
Step '/R relaunch: isolated wancode.exe process started after update' ($relaunched.Count -gt 0) `
    $(if ($relaunched.Count -gt 0) { "pid=" + ($relaunched[0].Id) + " path=" + $relaunched[0].Path } else { 'no process appeared within 20s' })
# kill ONLY isolated-dir processes
Get-IsoWancodeProcs | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch { } }

$foreign = @(Get-ForeignWancodeProcs)
Step 'No non-isolated wancode.exe was started or touched' ($foreign.Count -eq 0) ''

# ==================================== STEP 7: restore registry + final audit
Finish 0

} catch {
    Step 'Unhandled error (aborting safely)' $false ($_.Exception.Message + ' @ ' + $_.InvocationInfo.PositionMessage.Split("`n")[0])
    Finish 1
}
