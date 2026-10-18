<#
install.ps1 - make this Windows machine an iter engine (native Windows, no WSL).
ONE engine per machine; it serves every project the webui assigns to it.
iter_data runs elsewhere; this machine only makes outbound connections: to
iter_data, to Anthropic, to the projects' git remote and, once, to Hugging Face
for the embedding model. A Linux engine inside a WSL2 distro is
installers/iter_engine/linux_wsl instead.

  .\install.cmd [mode] [options]      (or: powershell -ExecutionPolicy Bypass -File .\install.ps1 ...)

Modes:
  install     (default) set up or repair; safe to run again
  status      the engine process, its logon task, its key, the served projects
  update      install the newer binary and restart (refuses while work runs, without -Yes)
  accounts    ask for each account token the served projects name
  projects    clone each served project that has no checkout yet
  start | stop    the engine process (the logon task runs `start`)
  logs        follow the engine log
  uninstall   stop the engine, remove its logon task, binary and PATH entry (keeps the key, tokens, model)

Options:
  -IterDir DIR     the folder that holds everything iter puts on this machine [C:\iter];
                   always ends in \iter: C:\my\dir\iter\, C:\my\dir\iter and C:\my\dir all give
                   C:\my\dir\iter (also $env:ITER_DIR; later runs find it from the logon task)
  -DataUrl URL     iter_data URL (else asked; kept in <iter dir>\iter_engine\config\engine.conf)
  -Name NAME       this engine's name, unique per machine (default: the hostname)
  -Version TAG     release to download, e.g. v2026.10.17 (else $env:ITER_VERSION, else latest)
  -Binary PATH     install this iter_engine.exe instead of downloading one
  -Fingerprint FP  iter_data's certificate fingerprint (sha256:..., from the iter_data
                   installer or the webui's Engine keys screen): pinned before sign-in
  -TrustCa         iter_data is behind a TLS proxy with a real certificate: check it
                   against certificate authorities instead of pinning
  -SecretsCommand CMD  a command (run by cmd /C) printing KEY=value lines: the account
                   tokens, from a secret store (aws ssm / secretsmanager, op, ...); the
                   engine runs it at startup and every -SecretsEvery minutes and keeps the
                   values in memory only. Kept in config\engine.conf and put on the
                   engine's command line; "none" removes it. With it, the accounts step
                   asks for no tokens.
  -SecretsEvery N  minutes between secrets-command runs [5]
  -Yes             answer yes to every question; update while work runs
  -DryRun          print every action, change nothing
  ($env:ITER_RELEASE_URL: download the binary and SHA256SUMS from this base URL instead)

The steps of `install`: tools (Git for Windows, whose bash runs the shell
steps; the claude CLI; asks before winget) -> iter_data URL + engine name
(config\engine.conf) -> iter_engine.exe into bin\ (the asset next to this
script, else the GitHub Release, sha256-checked; bin\ goes on the user PATH)
-> the embedding model into models\ -> the engine key (`iter_engine key new`:
paste its public key into the webui's Engine keys screen; the script waits
until iter_data accepts it) -> a laptop: no sleep on AC power (asks) -> the
logon task "iter engine" (runs bin\iter-engine.ps1 start, a copy of this
script) -> account tokens and project checkouts.

HTTPS: iter_data must be https:// (iter_data serves HTTPS by default; plain
http:// is refused unless iter_data runs on this machine: 127.0.0.1, localhost,
::1). The engine pins iter_data's certificate fingerprint in
config\server_trust.json: -Fingerprint (or the fingerprint pasted when asked)
runs `iter_engine key trust` right after the binary is installed and stops if
the server presents another certificate; without one, the first sign-in trusts
the certificate it sees (compare: iter_engine key status). Other key commands:
`iter_engine key show` (print the public key), `iter_engine key rotate` (a new
key, signed by the current one, no webui step).

Everything this machine keeps for iter is in <iter dir>\iter_engine (C:\iter\iter_engine):
  bin\       iter_engine.exe, iter-engine.ps1 (the logon task's copy of this script)
  config\    engine.conf, server_trust.json (iter_data's pinned certificate)
  secrets\   engine.key, .env (account tokens); only this user (and SYSTEM) may read it
  models\    the embedding model
  logs\      engine.log, engine.err.log, engine.pid
The engine runs with ITER_HOME set to that folder. An install from before
2026-10-18 in ~\.iter is moved there once (install, update); files the script
does not know go to old-dot-iter\, and ~\.iter is removed only when empty.
#>
param(
    [ValidateSet('install', 'status', 'update', 'accounts', 'projects', 'start', 'stop', 'logs', 'uninstall')]
    [string]$Mode = 'install',
    [Alias('Iter_Dir')]
    [string]$IterDir = '',
    [string]$DataUrl = '',
    [string]$Name = '',
    [string]$Version = $env:ITER_VERSION,
    [string]$Binary = '',
    [string]$Fingerprint = '',
    [switch]$TrustCa,
    [string]$SecretsCommand = '',
    [int]$SecretsEvery = 0,
    [switch]$Yes,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$script:SecretsGiven = $PSBoundParameters.ContainsKey('SecretsCommand')
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is many times slower with its progress bar
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { }
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$RepoSlug = 'Stephen-Hilton/iter'
$Asset = 'iter_engine-windows-x86_64.exe'   # also on ARM64 Windows, through its x64 emulation
$Here = $PSScriptRoot
$TaskName = 'iter engine'
$OldHome = Join-Path $HOME '.iter'   # the engine's folder before 2026-10-18

# -IterDir: the folder that holds everything, always ending in \iter
# (C:\my\dir\iter\, C:\my\dir\iter and C:\my\dir all give C:\my\dir\iter)
function Resolve-IterDir([string]$d) {
    $d = $d.Trim()
    if ($d -eq '~' -or $d -match '^~[\\/]') { $d = $HOME + $d.Substring(1) }
    # relative to the current location; the folder need not exist yet
    $d = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($d)
    while ($d.Length -gt 1 -and ($d.EndsWith('\') -or $d.EndsWith('/')) -and $d -notmatch '^[A-Za-z]:[\\/]$') { $d = $d.Substring(0, $d.Length - 1) }
    if ((Split-Path -Leaf $d) -ne 'iter') { $d = Join-Path $d 'iter' }
    return $d
}

# the iter dir of an existing install: this script running as the logon task's copy
# (<iter dir>\iter_engine\bin\iter-engine.ps1), else the task's -File argument; '' if none
function Find-InstalledIterDir {
    $paths = @()
    if ($PSCommandPath) { $paths += $PSCommandPath }
    $t = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($t -and $t.Actions -and "$($t.Actions[0].Arguments)" -match '-File\s+"([^"]+)"') { $paths += $Matches[1] }
    foreach ($p in $paths) {
        $bin = Split-Path $p
        $eng = Split-Path $bin
        if ((Split-Path -Leaf $p) -eq 'iter-engine.ps1' -and (Split-Path -Leaf $bin) -eq 'bin' -and (Split-Path -Leaf $eng) -eq 'iter_engine') { return (Split-Path $eng) }
    }
    return ''
}

$script:Installed = Find-InstalledIterDir
$Root = if ($IterDir) { Resolve-IterDir $IterDir }
        elseif ($env:ITER_DIR) { Resolve-IterDir $env:ITER_DIR }
        elseif ($script:Installed) { $script:Installed }
        else { Join-Path "$env:SystemDrive\" 'iter' }
$IterHome = Join-Path $Root 'iter_engine'
# iter_engine finds its files (key, server_trust.json, .env) through $env:ITER_HOME:
# the engine process gets it from Start-Engine, and this script's own calls too
$env:ITER_HOME = $IterHome
$BinDir = Join-Path $IterHome 'bin'
$ConfDir = Join-Path $IterHome 'config'
$SecretsDir = Join-Path $IterHome 'secrets'
$LogDir = Join-Path $IterHome 'logs'
$Exe = Join-Path $BinDir 'iter_engine.exe'
$Launcher = Join-Path $BinDir 'iter-engine.ps1'
$Conf = Join-Path $ConfDir 'engine.conf'
$TrustFile = Join-Path $ConfDir 'server_trust.json'
$EnvFile = Join-Path $SecretsDir '.env'
$KeyFile = Join-Path $SecretsDir 'engine.key'
$PidFile = Join-Path $LogDir 'engine.pid'
$Log = Join-Path $LogDir 'engine.log'
$ErrLog = Join-Path $LogDir 'engine.err.log'
$ModelDir = Join-Path $IterHome 'models\all-MiniLM-L6-v2'
$script:Engine = ''
$script:SecretsCmd = ''
$script:SecretsEvery = ''

function Say($m) { Write-Host "`n==> $m" }
function Ok($m) { Write-Host "    ok  $m" -ForegroundColor Green }
function Warn($m) { Write-Host "    !!  $m" -ForegroundColor Yellow }
function Dry($m) { Write-Host "    [dry-run] $m" -ForegroundColor Cyan }
function Ask($q, $default) {
    if ($Yes) { return $default }
    $a = Read-Host "    $($q.TrimEnd(':'))"
    if ([string]::IsNullOrWhiteSpace($a)) { return $default }
    return $a.Trim()
}
function YesNo($q) { if ($Yes) { return $true } return (Ask "$q [Y/n]" 'y') -notmatch '^[nN]' }
# run a step, or under -DryRun only describe it
function Step([string]$what, [scriptblock]$do) { if ($DryRun) { Dry $what } else { & $do } }

# Native tools write progress to stderr; Windows PowerShell 5.1 must not treat that as an error.
function Invoke-Native([string]$what, [scriptblock]$cmd) {
    if ($DryRun) { Dry $what; return }
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try {
        & $cmd 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) { "    $($_.Exception.Message)" } else { "    $_" }
        } | Out-Host
    } finally { $ErrorActionPreference = $eap }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    foreach ($d in @(Join-Path $HOME '.local\bin')) { if ((Test-Path $d) -and ($env:Path -notlike "*$d*")) { $env:Path = "$d;$env:Path" } }
}

function Winget-Install($id, $what) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw "$what is missing and winget is not available: install it by hand" }
    if (-not (YesNo "install $what with winget ($id)?")) { throw "$what is needed" }
    Invoke-Native "winget install $id" { winget install --id $id -e --accept-source-agreements --accept-package-agreements }
    Refresh-Path
}

function Get-Conf($key) {
    if (-not (Test-Path $Conf)) { return '' }
    $l = Get-Content $Conf | Where-Object { $_ -match "^$key=" } | Select-Object -Last 1
    if ($l) { return ($l -replace "^$key=", '') } return ''
}

function Get-EnvValue($key) {
    if (-not (Test-Path $EnvFile)) { return '' }
    $l = Get-Content $EnvFile | Where-Object { $_ -match "^\s*(export\s+)?$key\s*=" } | Select-Object -Last 1
    if ($l) { return ($l -replace "^\s*(export\s+)?$key\s*=\s*", '').Trim().Trim('"', "'") } return ''
}

# -SecretsCommand / -SecretsEvery: given now, else kept in engine.conf
function Load-Secrets {
    $script:SecretsCmd = if ($script:SecretsGiven) { $SecretsCommand } else { Get-Conf SECRETS_COMMAND }
    if ($script:SecretsCmd -eq 'none') { $script:SecretsCmd = '' }
    $script:SecretsEvery = if ($SecretsEvery -gt 0) { "$SecretsEvery" } else { Get-Conf SECRETS_EVERY }
    if ($script:SecretsEvery -notmatch '^[1-9][0-9]*$' -or -not $script:SecretsCmd) { $script:SecretsEvery = '' }
}

# one argument for a Windows command line (CommandLineToArgvW quoting)
function ConvertTo-WinArg([string]$a) {
    if ($a -and $a -notmatch '[\s"]') { return $a }
    $b = $a -replace '(\\*)"', '$1$1\"'
    $b = $b -replace '(\\+)$', '$1$1'
    return '"' + $b + '"'
}

function Load-Conf {
    Load-Secrets
    if (-not $script:DataUrl) { $script:DataUrl = Get-Conf DATA_URL }
    if (-not $script:Engine) { $script:Engine = if ($Name) { $Name } else { Get-Conf ENGINE } }
    if (-not $script:Engine) { throw 'not installed yet: run install.cmd' }
}

# a session from the engine key, for this script's own API calls ('' when there is none)
function Get-Bearer {
    if ((Test-Path $KeyFile) -and (Test-Path $Exe)) {
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        try { $t = & $Exe key session --key-file $KeyFile --data-url $script:DataUrl 2>$null } finally { $ErrorActionPreference = $eap }
        if ($LASTEXITCODE -eq 0 -and $t) { return "$t".Trim() }
    }
    return ''
}

# ------------------------------------------------------------------ iter_data over HTTPS
# iter_data serves HTTPS with its own (self-signed) certificate. The engine pins
# that certificate's fingerprint in config\server_trust.json (one entry per
# scheme://host:port), and plain http:// is only for this machine. This script's
# own API calls check the certificate the same way (IterPinnedGet below).

# the trust file's key for a URL: scheme://host:port, lowercased
function Get-TrustOrigin([string]$u) {
    $u = $u.Trim().TrimEnd('/')
    $i = $u.IndexOf('://')
    $s = if ($i -ge 0) { $u.Substring(0, $i).ToLower() } else { 'http' }
    $r = if ($i -ge 0) { $u.Substring($i + 3) } else { $u }
    $hp = ($r -split '[/?#]', 2)[0].ToLower()
    $hasPort = if ($hp.Contains(']')) { $hp.Substring($hp.LastIndexOf(']')).Contains(':') } else { $hp.Contains(':') }
    if (-not $hasPort) { $hp = $hp + ':' + $(if ($s -eq 'https') { '443' } else { '80' }) }
    return $s + '://' + $hp
}

function Test-LoopbackUrl([string]$u) {
    try { $h = ([Uri]$u).Host.Trim('[', ']').ToLower() } catch { return $false }
    if ($h -eq 'localhost') { return $true }
    $ip = $null
    if ([Net.IPAddress]::TryParse($h, [ref]$ip)) { return [Net.IPAddress]::IsLoopback($ip) }
    return $false
}

# this URL's entry in server_trust.json (mode, pins), or $null
function Get-TrustEntry([string]$u) {
    if (-not (Test-Path $TrustFile)) { return $null }
    try { $j = Get-Content $TrustFile -Raw | ConvertFrom-Json } catch { return $null }
    if (-not $j.servers) { return $null }
    $p = $j.servers.PSObject.Properties[(Get-TrustOrigin $u)]
    if ($p) { return $p.Value } return $null
}

# GET with a bearer token over HttpWebRequest, accepting only a certificate whose
# SHA-256 (of its DER) is one of the pins: the same check the engine makes. Empty
# pins = ordinary certificate-authority checks. Throws when it cannot.
$PinnedGetSource = @'
using System;
using System.IO;
using System.Net;
using System.Net.Security;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
public static class IterPinnedGet {
    public static string Get(string url, string bearer, string[] pins, int timeoutMs) {
        HttpWebRequest req = (HttpWebRequest)WebRequest.Create(url);
        req.Timeout = timeoutMs;
        req.ReadWriteTimeout = timeoutMs;
        if (!String.IsNullOrEmpty(bearer)) { req.Headers["Authorization"] = "Bearer " + bearer; }
        if (pins != null && pins.Length > 0) {
            req.ServerCertificateValidationCallback = delegate(object sender, X509Certificate cert, X509Chain chain, SslPolicyErrors errors) {
                if (cert == null) { return false; }
                string hex;
                using (SHA256 sha = SHA256.Create()) { hex = BitConverter.ToString(sha.ComputeHash(cert.GetRawCertData())).Replace("-", "").ToLowerInvariant(); }
                foreach (string p in pins) { if (p == hex) { return true; } }
                return false;
            };
        }
        using (HttpWebResponse resp = (HttpWebResponse)req.GetResponse())
        using (StreamReader rd = new StreamReader(resp.GetResponseStream(), System.Text.Encoding.UTF8)) { return rd.ReadToEnd(); }
    }
}
'@

# GET an iter_data path as this engine, iter_data's certificate checked as the engine
# checks it; $null on failure (unreachable, refused, nothing trusted yet, or another
# certificate). Read-only.
function Get-Api($path) {
    $ErrorActionPreference = 'Continue'
    $tok = Get-Bearer
    if (-not $tok) { return $null }
    $pins = @()
    if ($script:DataUrl -match '^https://') {
        $t = Get-TrustEntry $script:DataUrl
        if (-not $t) { return $null }
        if ($t.mode -ne 'ca') {
            $pins = @($t.pins | ForEach-Object { ("$_" -replace '^sha256:', '' -replace ':', '').ToLower() })
            if ($pins.Count -eq 0) { return $null }
        }
    }
    try {
        if (-not ('IterPinnedGet' -as [type])) { Add-Type -TypeDefinition $PinnedGetSource -IgnoreWarnings -WarningAction SilentlyContinue }
        return [IterPinnedGet]::Get("$($script:DataUrl)$path", $tok, [string[]]$pins, 15000) | ConvertFrom-Json
    } catch { return $null }
}

# Git for Windows' bash.exe, found from git.exe (<Git>\cmd\git.exe -> <Git>\bin\bash.exe),
# as iter_core::platform::git_bash finds it; $null when there is none
function Find-GitBash {
    $git = Get-Command git -ErrorAction SilentlyContinue
    if (-not $git) { return $null }
    $dir = Split-Path $git.Source
    foreach ($root in (Split-Path $dir), (Split-Path (Split-Path $dir))) {
        if (-not $root) { continue }
        $b = Join-Path $root 'bin\bash.exe'
        if (Test-Path $b) { return $b }
    }
    return $null
}

function Expand-Topdir($t) { if ($t -match '^~[\\/]') { return (Join-Path $HOME $t.Substring(2)) } return $t }
function Sha256($f) { return (Get-FileHash -Algorithm SHA256 $f).Hash.ToLower() }
function Sum-For($sumsFile, $asset) {
    foreach ($l in Get-Content $sumsFile) {
        $p = $l.Trim() -split '\s+'
        if ($p.Count -ge 2 -and ($p[1] -eq $asset -or $p[1] -eq "*$asset")) { return $p[0].ToLower() }
    }
    return ''
}

# ------------------------------------------------------------------ the iter dir

# bin\ config\ secrets\ models\ logs\ under <iter dir>\iter_engine; secrets\ readable
# by this user (and SYSTEM) only
function Initialize-Layout {
    if ($DryRun) { Dry "create $IterHome\{bin,config,secrets,models,logs}; secrets\ for $env:USERNAME (and SYSTEM) only"; return }
    try { foreach ($d in $BinDir, $ConfDir, $SecretsDir, $LogDir, (Join-Path $IterHome 'models')) { New-Item -ItemType Directory -Force $d | Out-Null } }
    catch { throw "cannot create ${IterHome}: $($_.Exception.Message) (choose another place with -IterDir, or create $Root as an administrator and give $env:USERNAME write access)" }
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    Invoke-Native "icacls $SecretsDir (owner only)" { icacls $SecretsDir /inheritance:r /grant:r "*${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' /Q }
    foreach ($f in Get-ChildItem $SecretsDir -Force -File) {
        Invoke-Native "icacls $($f.FullName) (owner only)" { icacls $f.FullName /inheritance:r /grant:r "*${sid}:F" '*S-1-5-18:F' /Q }
    }
    Ok "$IterHome (secrets\ for $env:USERNAME only)"
}

# move a file or folder into $dest (created); when $dest already has one of that
# name it goes to old-dot-iter\ instead, so nothing is overwritten or deleted
# The older folder is the live install (a finished move leaves it empty), so its
# file takes the place: what is in the way (left from an earlier install) is set
# aside in replaced-<time>\. Nothing is overwritten or deleted.
$Stamp = Get-Date -Format 'yyyyMMddHHmmss'
function Move-Into($item, [string]$dest) {
    $target = Join-Path $dest $item.Name
    if (Test-Path -LiteralPath $target) {
        $aside = Join-Path $IterHome "replaced-$Stamp"
        if ($dest -eq (Join-Path $IterHome 'old-dot-iter')) { $target = "$target.$Stamp" }
        elseif ($DryRun) { Dry "set aside $target -> $aside\" }
        else {
            New-Item -ItemType Directory -Force $aside | Out-Null
            Move-Item -LiteralPath $target -Destination (Join-Path $aside $item.Name)
            Warn "$target was there already (from an earlier install): set aside in $aside"
        }
    }
    if ($DryRun) { Dry "move $($item.FullName) -> $target"; return }
    New-Item -ItemType Directory -Force $dest | Out-Null
    Move-Item -LiteralPath $item.FullName -Destination $target
}

# an install from before 2026-10-18 kept everything flat in ~\.iter: each file goes
# to its folder here, what the script does not know to old-dot-iter\, and ~\.iter
# is removed only once it is empty
function Move-OldHome {
    if (-not (Test-Path $OldHome -PathType Container)) { return }
    Say "moving the old $OldHome into $IterHome"
    Stop-Engine (Join-Path $OldHome 'engine.pid')
    $script:Relocated = $true   # the logon task still names ~\.iter: registered again
    foreach ($i in Get-ChildItem $OldHome -Force) {
        switch -Wildcard ($i.Name) {
            'bin' { foreach ($b in Get-ChildItem $i.FullName -Force) { Move-Into $b $BinDir } }
            'logs' { foreach ($l in Get-ChildItem $i.FullName -Force) { Move-Into $l $LogDir } }
            'models' { foreach ($m in Get-ChildItem $i.FullName -Force) { Move-Into $m (Join-Path $IterHome 'models') } }
            # a newer iter_engine already wrote here (pinned certificates)
            'config' { foreach ($c in Get-ChildItem $i.FullName -Force) { Move-Into $c $ConfDir } }
            'secrets' { foreach ($c in Get-ChildItem $i.FullName -Force) { Move-Into $c $SecretsDir } }
            'engine.conf' { Move-Into $i $ConfDir }
            'server_trust.json' { Move-Into $i $ConfDir }
            'engine.key*' { Move-Into $i $SecretsDir }
            '.env' { Move-Into $i $SecretsDir }
            'engine*.log' { Move-Into $i $LogDir }
            'engine.pid' { Move-Into $i $LogDir }
            default { Move-Into $i (Join-Path $IterHome 'old-dot-iter') }
        }
    }
    if ($DryRun) { Dry "remove $OldHome once it is empty"; return }
    foreach ($d in 'bin', 'logs', 'models', 'config', 'secrets') {
        $p = Join-Path $OldHome $d
        if ((Test-Path $p) -and -not (Get-ChildItem $p -Force)) { Remove-Item $p }
    }
    if (Get-ChildItem $OldHome -Force) { Warn "$OldHome still holds files (left as they were)" }
    else { Remove-Item $OldHome; Ok "moved $OldHome to $IterHome (files it did not know: old-dot-iter\)" }
}

# installed at another iter dir than the one asked for: stop the engine there and
# move its folder here (the logon task is registered again for the new place)
function Move-IterDir {
    if (-not $script:Installed -or $script:Installed -eq $Root) { return }
    $old = Join-Path $script:Installed 'iter_engine'
    if (-not (Test-Path $old)) { return }
    if (Test-Path $IterHome) { throw "the engine is installed in $old, and $IterHome exists too: move what you need by hand, or run with -IterDir $($script:Installed)" }
    Say "moving the engine from $old to $IterHome"
    Stop-Engine (Join-Path $old 'logs\engine.pid')
    Remove-UserPath (Join-Path $old 'bin')
    $script:Relocated = $true
    if ($DryRun) { Dry "move $old -> $IterHome"; return }
    New-Item -ItemType Directory -Force $Root | Out-Null
    if ([IO.Path]::GetPathRoot($old) -eq [IO.Path]::GetPathRoot($IterHome)) { Move-Item -LiteralPath $old -Destination $IterHome }
    else { Copy-Item -LiteralPath $old -Destination $IterHome -Recurse; Remove-Item -LiteralPath $old -Recurse -Force }   # another drive: copy, then the old copy goes
    if (-not (Get-ChildItem $script:Installed -Force)) { Remove-Item $script:Installed }
    Ok "moved to $IterHome"
}

function Get-UserPathPart { return @("$([Environment]::GetEnvironmentVariable('Path', 'User'))" -split ';' | Where-Object { $_ }) }
# bin\ on the user PATH, once
function Add-UserPath([string]$dir) {
    $parts = Get-UserPathPart
    if ($parts | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }) { Ok "$dir is on the user PATH"; return }
    Step "add $dir to the user PATH" { [Environment]::SetEnvironmentVariable('Path', ((@($parts) + $dir) -join ';'), 'User'); $env:Path = "$dir;$env:Path" }
    Ok "$dir on the user PATH (new shells find iter_engine)"
}
function Remove-UserPath([string]$dir) {
    $parts = Get-UserPathPart
    $keep = @($parts | Where-Object { $_.TrimEnd('\') -ine $dir.TrimEnd('\') })
    if ($keep.Count -eq $parts.Count) { return }
    Step "remove $dir from the user PATH" { [Environment]::SetEnvironmentVariable('Path', ($keep -join ';'), 'User') }
}

# ------------------------------------------------------------------ the engine process

function Get-EngineProcess([string]$pidFile = $PidFile) {
    if (-not (Test-Path $pidFile)) { return $null }
    $p = Get-Process -Id ([int](Get-Content $pidFile -Raw).Trim()) -ErrorAction SilentlyContinue
    if ($p -and $p.ProcessName -eq 'iter_engine') { return $p }
    return $null
}

function Stop-Engine([string]$pidFile = $PidFile) {
    $p = Get-EngineProcess $pidFile
    if (-not $p) { return }
    if ($DryRun) { Dry "stop iter_engine (pid $($p.Id)) and its agent sessions (taskkill /T)"; return }
    Say "stopping iter_engine (pid $($p.Id))"
    # /T: the claude sessions and shells it started go with it
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    taskkill /T /F /PID $p.Id 2>&1 | Out-Null
    $ErrorActionPreference = $eap
    $p.WaitForExit(10000) | Out-Null
    Remove-Item $pidFile -ErrorAction SilentlyContinue
}

function Start-Engine {
    Load-Conf
    if (-not $DryRun -and -not (Test-Path $Exe)) { throw "$Exe is missing: run install.cmd" }
    if (-not (Test-Path $KeyFile)) {
        if ($DryRun) { Dry "start iter_engine (once $KeyFile exists)"; return }
        throw "no engine key at ${KeyFile}: run install.cmd (or '$Exe key new', then add its public key in the webui's Engine keys screen)"
    }
    $running = Get-EngineProcess
    if ($running) { Ok "iter_engine already running (pid $($running.Id))"; return }
    $engineArgs = @('--data-url', $script:DataUrl, '--env-file', $EnvFile, '--key-file', $KeyFile, '--name', $script:Engine)
    if ($script:SecretsCmd) { $engineArgs += @('--secrets-command', $script:SecretsCmd) }
    if ($script:SecretsEvery) { $engineArgs += @('--secrets-every', $script:SecretsEvery) }
    # one quoted command line: Start-Process joins an argument array without quoting
    $engineArgs = ($engineArgs | ForEach-Object { ConvertTo-WinArg $_ }) -join ' '
    if ($DryRun) { Dry "start (hidden) $Exe $engineArgs > $Log 2> $ErrLog"; return }
    New-Item -ItemType Directory -Force $LogDir | Out-Null
    $env:ITER_HOME = $IterHome   # the engine finds its key, trust file and .env here
    $p = Start-Process -FilePath $Exe -ArgumentList $engineArgs -WorkingDirectory $IterHome -WindowStyle Hidden `
        -RedirectStandardOutput $Log -RedirectStandardError $ErrLog -PassThru
    Set-Content $PidFile $p.Id
    Ok "iter_engine '$($script:Engine)' started (pid $($p.Id)) -> $($script:DataUrl); log $Log"
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep 1
        if ($p.HasExited) { Get-Content $ErrLog -Tail 20 -ErrorAction SilentlyContinue; throw "iter_engine exited ($($p.ExitCode))" }
        $e = Get-Api "/api/engines/$($script:Engine)"
        if ($e -and $e.state -eq 'Running') { Ok "engine checked in: state $($e.state)"; return }
    }
    Warn "no Running check-in after 30s; see $Log and $ErrLog"
}

# ------------------------------------------------------------------ the binary

# the file to install: -Binary, the asset next to this script, else the release
function Get-NewBinary {
    if ($Binary) {
        if (-not (Test-Path $Binary)) { throw "-Binary ${Binary}: no such file" }
        Ok "using $Binary"; return (Resolve-Path $Binary).Path
    }
    if ($Here -and (Test-Path (Join-Path $Here $Asset))) {
        $f = Join-Path $Here $Asset
        $sums = Join-Path $Here 'SHA256SUMS'
        if (Test-Path $sums) {
            $want = Sum-For $sums $Asset
            if ($want -and (Sha256 $f) -ne $want) { throw "$f does not match its SHA256SUMS entry" }
        }
        Ok "using $f (next to this script)"; return $f
    }
    $base = if ($env:ITER_RELEASE_URL) { $env:ITER_RELEASE_URL.TrimEnd('/') }
            elseif ($Version) { "https://github.com/$RepoSlug/releases/download/$Version" }
            else { "https://github.com/$RepoSlug/releases/latest/download" }
    if ($DryRun) { Dry "download $base/$Asset and $base/SHA256SUMS, check its sha256"; return '' }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "iter-install-$PID"
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $f = Join-Path $tmp $Asset; $sums = Join-Path $tmp 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing "$base/$Asset" -OutFile $f
    Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile $sums
    $want = Sum-For $sums $Asset; $got = Sha256 $f
    if (-not $want) { throw "SHA256SUMS of that release has no $Asset" }
    if ($got -ne $want) { throw "$Asset sha256 $got does not match the release's $want" }
    Ok "downloaded $Asset $(if ($Version) { $Version } else { '(latest)' }), sha256 ok"
    return $f
}

# install it as bin\iter_engine.exe; true when the file changed. Windows
# will not overwrite a running .exe: the engine is stopped first (and started
# again by the caller).
function Install-Binary {
    $new = Get-NewBinary
    if ($DryRun -or -not $new) { Dry "install it as $Exe (stop the engine, temp file + move)"; return $true }
    if ((Test-Path $Exe) -and (Sha256 $Exe) -eq (Sha256 $new)) { Ok "$Exe is already this build"; return $false }
    New-Item -ItemType Directory -Force $BinDir | Out-Null
    $tmp = Join-Path $BinDir "iter_engine.new.$PID.exe"
    Copy-Item $new $tmp -Force
    Unblock-File $tmp -ErrorAction SilentlyContinue
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    & $tmp --help *> $null; $rc = $LASTEXITCODE
    $ErrorActionPreference = $eap
    if ($rc -ne 0) { Remove-Item $tmp -Force; throw "$Asset does not run here (exit $rc)" }
    Stop-Engine
    Move-Item $tmp $Exe -Force
    Ok $Exe
    return $true
}

# ------------------------------------------------------------------ steps

function Step-Tools {
    Say '1/8 tools'
    Refresh-Path
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Winget-Install 'Git.Git' 'Git for Windows' }
    if (-not $DryRun -or (Get-Command git -ErrorAction SilentlyContinue)) {
        Ok "git $((git --version) -replace 'git version ', '')"
        # Windows-native: shell steps run in Git for Windows' bash, never WSL's
        # (a bare `bash` on Windows can be System32\bash.exe, the WSL launcher)
        $b = Find-GitBash
        if (-not $b) { throw "Git for Windows' bash.exe was not found beside git.exe: reinstall Git for Windows (winget install Git.Git)" }
        Ok "Git Bash $b (shell steps; WSL is never used)"
    }
    if (Get-Command claude -ErrorAction SilentlyContinue) { Ok "claude $(cmd /c 'claude --version 2>nul' | Select-Object -First 1)" }
    elseif (YesNo "install the claude CLI (Anthropic's installer)?") {
        Step "Invoke-RestMethod https://claude.ai/install.ps1 | Invoke-Expression" {
            Invoke-Expression (Invoke-RestMethod 'https://claude.ai/install.ps1'); Refresh-Path
            if (Get-Command claude -ErrorAction SilentlyContinue) { Ok 'claude installed' } else { Warn 'claude is not on PATH yet: open a new PowerShell' }
        }
    } else { Warn 'agents cannot run until the claude CLI is installed' }
    if (-not (Get-Command pdftoppm -ErrorAction SilentlyContinue)) { Warn 'pdftoppm (poppler) not found: GraphRAG skips PDF pages until it is on PATH' }
}

function Step-Settings {
    $ErrorActionPreference = 'Continue'
    Say '2/8 iter_data and this engine''s name'
    $prior = Get-Conf DATA_URL
    $url = if ($DataUrl) { $DataUrl } else { Ask "iter_data URL [$(if ($prior) { $prior } else { 'https://iter.example.com' })]:" $prior }
    $url = "$url".TrimEnd('/')
    if (-not $url) {
        if (-not $DryRun) { throw 'the iter_data URL is required (-DataUrl URL)' }
        $url = 'https://iter.example.com'; Warn "no iter_data URL given: the dry run goes on with $url"
    }
    if ($url -notmatch '^https?://') { throw "${url}: the iter_data URL starts with https:// (plain http:// only to 127.0.0.1 / localhost / ::1)" }
    if ($url -match '^http://' -and -not (Test-LoopbackUrl $url)) {
        throw "$url is plain http: iter_data must be reached over https:// (iter_data serves HTTPS by default), so the engine's session token and the work never cross the network unencrypted; plain http:// is only for iter_data on this machine (127.0.0.1, localhost, ::1). Try $($url -replace '^http://', 'https://')"
    }
    # -k: only "does it answer?"; the certificate is checked by iter_engine (pinned)
    & curl.exe -sS -f -k --max-time 10 -o NUL "$url/health" 2>$null
    if ($LASTEXITCODE -eq 0) { Ok "$url answers" } else { Warn "no answer from $url/health (a proxy may hide it; the key check below tells)" }
    # hostname keeps its case; $env:COMPUTERNAME is upper-cased and would register a second engine
    $priorName = Get-Conf ENGINE
    $def = if ($priorName) { $priorName } else { hostname }
    $n = if ($Name) { $Name } else { Ask "engine name [$def]:" $def }
    if ($n -notmatch '^[A-Za-z0-9_.-]+$') { throw "engine name '$n': letters, digits, - _ . only" }
    Load-Secrets
    $lines = @("DATA_URL=$url", "ENGINE=$n")
    if ($script:SecretsCmd) {
        $lines += "SECRETS_COMMAND=$($script:SecretsCmd)"
        Ok "account tokens: from the secrets command, every $(if ($script:SecretsEvery) { $script:SecretsEvery } else { 5 }) min (values stay in the engine's memory)"
    }
    if ($script:SecretsEvery) { $lines += "SECRETS_EVERY=$($script:SecretsEvery)" }
    Step "write $Conf ($($lines -join ', '))" {
        New-Item -ItemType Directory -Force $ConfDir | Out-Null
        [IO.File]::WriteAllLines($Conf, [string[]]$lines)
    }
    $script:DataUrl = $url; $script:Engine = $n
    Ok "engine '$n' -> $url (in $Conf)"
    if (Get-ScheduledTask -TaskName 'iter startup' -ErrorAction SilentlyContinue) {
        Warn "the logon task 'iter startup' (deploy.ps1 startup) also starts a Windows engine from the repo; with this install, let it start only the WSL server distro"
    }
}

function Step-Binary {
    Say '3/8 iter_engine'
    $script:Changed = Install-Binary
    Add-UserPath $BinDir
}

# iter_data's certificate, before the first sign-in: -Fingerprint (or the one pasted
# here) pins it now and stops when the server presents another one; -TrustCa checks
# it against certificate authorities (a TLS proxy with a real certificate); else the
# first sign-in trusts what it sees (trust on first use).
function Step-Trust {
    $ErrorActionPreference = 'Continue'
    Say '3b/8 iter_data''s certificate'
    if ($script:DataUrl -notmatch '^https://') { Ok "$($script:DataUrl) is plain http to this machine: no certificate to trust"; return }
    if ($TrustCa) {
        Invoke-Native "$Exe key trust --data-url $($script:DataUrl) --ca" { & $Exe key trust --data-url $script:DataUrl --ca }
        return
    }
    $fp = $Fingerprint
    $has = [bool](Get-TrustEntry $script:DataUrl)
    if (-not $fp -and -not $has) {
        $keyIcon = [char]::ConvertFromUtf32(0x1F511)
        if ($DryRun) { Dry "ask for iter_data's certificate fingerprint (Enter = trust it at the first sign-in)" }
        elseif (-not $Yes) { $fp = Ask "Paste iter_data's certificate fingerprint (shown in the webui's $keyIcon Engine keys screen, or by the iter_data installer), or press Enter to trust it at the first sign-in:" '' }
    }
    if (-not $fp) {
        if ($has) { Ok "trusted already ($TrustFile)" }
        else { Warn "no fingerprint given: the first sign-in trusts the certificate $($script:DataUrl) presents; compare it afterwards (iter_engine key status) with the webui's Engine keys screen" }
        return
    }
    try {
        Invoke-Native "$Exe key trust --data-url $($script:DataUrl) --fingerprint $fp" { & $Exe key trust --data-url $script:DataUrl --fingerprint $fp }
    } catch {
        throw "$($script:DataUrl) does not present the certificate ${fp}: something else answers at that URL (or the fingerprint is mistyped). Nothing was trusted; check the URL and the fingerprint (iter_data installer output, webui Engine keys screen)"
    }
}

# the embedding model (sentence-transformers/all-MiniLM-L6-v2, Apache-2.0,
# ~91 MB), checksum-verified; the engine finds models\ beside its
# bin\. Without it the engine downloads the model itself on first use.
function Step-Model {
    Say '4/8 embedding model'
    $base = 'https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main'
    $msum = '53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db'
    $st = Join-Path $ModelDir 'model.safetensors'
    if ((Test-Path $st) -and (Test-Path (Join-Path $ModelDir 'tokenizer.json')) -and (Test-Path (Join-Path $ModelDir 'config.json')) -and (Sha256 $st) -eq $msum) { Ok $ModelDir; return }
    if ($DryRun) { Dry "download all-MiniLM-L6-v2 from $base into $ModelDir, check its sha256"; return }
    New-Item -ItemType Directory -Force (Join-Path $ModelDir '1_Pooling') | Out-Null
    foreach ($f in 'config.json', 'tokenizer.json', 'model.safetensors', 'special_tokens_map.json', 'tokenizer_config.json',
                   'modules.json', 'sentence_bert_config.json', '1_Pooling/config.json', 'README.md') {
        $out = Join-Path $ModelDir ($f -replace '/', '\')
        try { Invoke-WebRequest -UseBasicParsing "$base/$f" -OutFile "$out.part"; Move-Item "$out.part" $out -Force }
        catch { Warn "could not fetch ${f}: the engine downloads the model itself later"; return }
    }
    if ((Sha256 $st) -ne $msum) { Warn 'model.safetensors differs from the verified build (upstream changed?)' }
    Ok $ModelDir
}

function Step-Key {
    $ErrorActionPreference = 'Continue'
    Say '5/8 engine key'
    if ($DryRun) {
        if (Test-Path $KeyFile) { Ok "$KeyFile exists" } else { Dry "$Exe key new --key-file $KeyFile --name $($script:Engine)" }
        Dry "wait until iter_data accepts it ($Exe key status)"; return
    }
    New-Item -ItemType Directory -Force $SecretsDir | Out-Null
    if (-not (Test-Path $EnvFile)) { New-Item -ItemType File $EnvFile | Out-Null }
    if (-not (Test-Path $KeyFile)) { & $Exe key new --key-file $KeyFile --name $script:Engine | ForEach-Object { "    $_" } }
    $first = $true
    while ($true) {
        $out = & $Exe key status --key-file $KeyFile --data-url $script:DataUrl
        if ($LASTEXITCODE -eq 0) { $out | ForEach-Object { "    $_" }; Ok "iter_data accepts this engine's key"; return }
        if ($first) {
            $out | ForEach-Object { "    $_" }
            Write-Host "`n    In the webui ($($script:DataUrl)): Engine keys (the key button in the header) -> Add: paste the iter_... public key above, engine name '$($script:Engine)'."
            $first = $false
        }
        if ($Yes) { Warn 'not accepted yet (-Yes does not wait): add the key, then run this again'; return }
        switch (Ask 'press Enter once it is added (n = a new key, s = skip):' '') {
            { $_ -in 'n', 'N' } {
                Move-Item $KeyFile "$KeyFile.replaced-$(Get-Date -Format yyyyMMddHHmmss)" -Force
                & $Exe key new --key-file $KeyFile --name $script:Engine | ForEach-Object { "    $_" }; $first = $true
            }
            { $_ -in 's', 'S' } { Warn 'skipped: iter_data refuses this engine until its public key is added'; return }
            default { Start-Sleep 2; Write-Host '    not accepted yet' }
        }
    }
}

function Step-Power {
    Say '6/8 power'
    if (-not (Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue)) { Ok 'not a laptop'; return }
    if (YesNo 'this laptop runs agents unattended: never sleep on AC power, lid closed or not?') {
        Step 'powercfg: standby + hibernate timeout on AC = 0, lid action on AC = nothing' {
            powercfg /change standby-timeout-ac 0
            powercfg /change hibernate-timeout-ac 0
            powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 0
            powercfg /setactive SCHEME_CURRENT
        }
        Ok 'no sleep or hibernate on AC; closing the lid does nothing on AC'
    } else { Warn 'a sleeping laptop stops its agents mid-run' }
}

# a logon task runs a copy of this script (bin\iter-engine.ps1 start), so the
# engine starts at logon without the download or checkout it came from; the copy
# finds its iter dir from where it sits (and -IterDir says it too)
function Register-LogonTask {
    $taskArgs = "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Launcher`" start -IterDir `"$Root`""
    Step "register the logon task '$TaskName': powershell.exe $taskArgs" {
        $action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $taskArgs -WorkingDirectory $IterHome
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
        $set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -Hidden
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $set -Force `
            -Description "iter: start iter_engine '$($script:Engine)' against $($script:DataUrl)" | Out-Null
    }
    Ok "logon task '$TaskName'"
}

function Step-Service {
    Say '7/8 start at logon'
    if ($PSCommandPath -and ((Resolve-Path $PSCommandPath).Path -ne $Launcher)) {
        Step "copy this script to $Launcher" { New-Item -ItemType Directory -Force $BinDir | Out-Null; Copy-Item $PSCommandPath $Launcher -Force }
    } elseif (-not $PSCommandPath) { throw 'run this script from a file (not piped into Invoke-Expression): the logon task needs a copy of it' }
    Register-LogonTask
    # the conf may have changed: a running engine restarts with it
    if ($script:Changed -or (Get-EngineProcess)) { Stop-Engine }
    Start-Engine
}

function Step-Accounts {
    Say 'account tokens'
    $a = Get-Api "/api/engines/$($script:Engine)/assignments"
    if (-not $a -or -not $a.projects) { Ok "no projects served yet: claude agents use this machine's own login (run ``claude`` once and log in)"; return }
    $vars = @($a.accounts | ForEach-Object { $_.token_envar }) + @($a.projects | ForEach-Object { $_.accounts | ForEach-Object { $_.token_envar } }) |
        Where-Object { $_ } | Sort-Object -Unique
    if (-not $vars) { Ok 'the served projects name no account tokens'; return }
    if ($script:SecretsCmd) {
        Ok "the secrets command supplies the account tokens (not $EnvFile): nothing to paste here"
        if ($DryRun) { Dry "run the secrets command once and check it prints $($vars -join ', ') (names only)"; return }
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        $names = @(& cmd.exe /d /c $script:SecretsCmd 2>$null | ForEach-Object { if ("$_" -match '^\s*(export\s+)?([A-Za-z_][A-Za-z0-9_]*)=') { $Matches[2] } })
        $ErrorActionPreference = $eap
        foreach ($v in $vars) {
            if ($names -contains $v) { Ok "$v (from the secrets command)" }
            elseif (Get-EnvValue $v) { Ok "$v (from $EnvFile)" }
            else { Warn "${v}: the secrets command does not print it here (the engine logs missing token names)" }
        }
        return
    }
    foreach ($v in $vars) {
        if (Get-EnvValue $v) { Ok $v; continue }
        if ($DryRun) { Dry "ask for $v and add it to $EnvFile"; continue }
        $sec = Read-Host "    ${v}: paste a token from ``claude setup-token`` (run while logged in to that account; blank = later)" -AsSecureString
        $val = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec))
        if ($val) {
            $lines = @(if (Test-Path $EnvFile) { Get-Content $EnvFile | Where-Object { $_ -notmatch "^\s*$v\s*=" } }) + "$v=$val"
            [IO.File]::WriteAllLines($EnvFile, [string[]]$lines)
            Ok "$v (the engine picks it up without a restart)"
        } else { Warn "$v not set: that account is never used here" }
    }
}

function Step-Projects {
    Say 'project checkouts'
    $a = Get-Api "/api/engines/$($script:Engine)/assignments"
    if (-not $a -or -not $a.projects) {
        Ok "no projects served yet: in the webui's Settings tab draw a serves edge from '$($script:Engine)' to each project, topdir C:/Users/<you>/dev/<project>; then: install.cmd projects"
        return
    }
    foreach ($p in $a.projects) {
        if (-not $p.topdir) { Warn "$($p.project): its serves edge has no topdir (set one in the Settings tab)"; continue }
        $dir = Expand-Topdir $p.topdir
        if (Test-Path (Join-Path $dir '.git')) { Ok "$($p.project): $dir"; continue }
        if ((Test-Path $dir) -and (Get-ChildItem $dir -Force | Select-Object -First 1)) { Warn "$($p.project): $dir exists but is not a git checkout; left alone"; continue }
        $rec = Get-Api "/api/projects/$($p.project)"
        $url = if ($rec -and $rec.gitrepo) { $rec.gitrepo } else { '' }
        $url = Ask "$($p.project): git URL to clone into $dir [$(if ($url) { $url } else { 'none' })]:" $url
        if (-not $url) { Warn "$($p.project): no checkout (the engine skips it until $dir exists)"; continue }
        if ($DryRun) { Dry "git clone $url $dir"; continue }
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        & git clone -q $url $dir
        $ErrorActionPreference = $eap
        if ($LASTEXITCODE -eq 0) { Ok "$($p.project): cloned into $dir" } else { Warn "$($p.project): clone failed (credentials? try: gh auth login)" }
    }
}

function Show-Status {
    Load-Conf
    Write-Host "engine   $($script:Engine) -> $($script:DataUrl)"
    Write-Host "folder   $IterHome"
    $p = Get-EngineProcess
    if ($p) { Write-Host "process  running (pid $($p.Id), started $($p.StartTime))" } else { Write-Host 'process  not running' }
    $t = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($t) { Write-Host "task     '$TaskName' $($t.State)" } else { Write-Host "task     '$TaskName' missing" }
    if (Test-Path $Exe) {
        Write-Host "binary   $Exe (sha256 $((Sha256 $Exe).Substring(0, 12)))"
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        & $Exe key status --key-file $KeyFile --data-url $script:DataUrl
        $ErrorActionPreference = $eap
    } else { Write-Host "binary   $Exe missing" }
    $a = Get-Api "/api/engines/$($script:Engine)/assignments"
    if ($a -and $a.projects) { $a.projects | ForEach-Object { Write-Host "project  $($_.project)  $($_.topdir)  $($_.state)" } } else { Write-Host 'project  (none served)' }
    if (Test-Path $Log) { Get-Content $Log -Tail 3 }
}

function Do-Update {
    Load-Conf
    Say 'update iter_engine'
    $e = Get-Api "/api/engines/$($script:Engine)"
    if ($e -and $e.running -gt 0 -and -not $Yes) { throw "engine '$($script:Engine)' runs $($e.running) item(s) and a restart stops them: set its projects Draining and wait, or rerun with -Yes" }
    $wasRunning = $script:WasRunning -or [bool](Get-EngineProcess)
    $changed = Install-Binary
    if ($PSCommandPath -and (Test-Path $Launcher) -and ((Resolve-Path $PSCommandPath).Path -ne $Launcher)) {
        Step "refresh $Launcher" { Copy-Item $PSCommandPath $Launcher -Force }
    }
    if ($script:Relocated) { Register-LogonTask; Add-UserPath $BinDir }
    if (($changed -or $script:Relocated) -and $wasRunning) { Start-Engine }
}

function Do-Uninstall {
    Say 'uninstall the iter engine'
    Stop-Engine
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Step "unregister the logon task '$TaskName'" { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false }
        Ok "logon task '$TaskName' removed"
    }
    foreach ($f in $Exe, $Launcher) { if (Test-Path $f) { Step "remove $f" { Remove-Item $f -Force }; Ok "removed $f" } }
    Remove-UserPath $BinDir
    Ok "kept $IterHome (config\, secrets\, models\, logs\) and the project checkouts; delete them by hand when this machine is done with iter"
    Ok "in the webui, remove the engine's key (Engine keys) and its serves edges"
}

if ($DryRun) { Write-Host 'dry run: every action is printed, nothing is changed' }
$script:Relocated = $false
$script:WasRunning = $false
if ($Mode -in 'install', 'update') {
    # an engine running from the old place (or ~\.iter) is stopped by the move; update starts it again
    $script:WasRunning = [bool]((Get-EngineProcess (Join-Path $OldHome 'engine.pid')) -or
        ($script:Installed -and (Get-EngineProcess (Join-Path $script:Installed 'iter_engine\logs\engine.pid'))))
    Move-IterDir; Move-OldHome; Initialize-Layout
}
switch ($Mode) {
    'install' {
        Step-Tools; Step-Settings; Step-Binary; Step-Trust; Step-Model; Step-Key; Step-Power; Step-Service
        Say '8/8 accounts and projects'
        Step-Accounts; Step-Projects
        if ($DryRun) { Write-Host "`nDry run done: nothing was changed."; break }
        Write-Host "`nDone. Engine '$($script:Engine)' starts at logon and checks in with $($script:DataUrl)."
        Write-Host 'Next, in the webui''s Settings tab: draw a serves edge from it to each project (topdir with forward'
        Write-Host 'slashes, e.g. C:/Users/<you>/dev/<project>), then run install.cmd projects here to clone them.'
        Write-Host "  a new key: $Exe key rotate   (its public key: $Exe key show)"
    }
    'status' { Show-Status }
    'update' { Do-Update }
    'accounts' { Load-Conf; Step-Accounts }
    'projects' { Load-Conf; Step-Projects }
    'start' { Start-Engine }
    'stop' { Stop-Engine; Ok 'stopped' }
    'logs' { if ($DryRun) { Dry "Get-Content -Wait -Tail 50 $Log" } else { Get-Content $Log -Tail 50 -Wait } }
    'uninstall' { Do-Uninstall }
}
