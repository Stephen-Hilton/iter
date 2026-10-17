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
  uninstall   stop the engine, remove its logon task and binary (keeps ~\.iter's key, tokens, model)

Options:
  -DataUrl URL     iter_data URL (else asked; kept in ~\.iter\engine.conf)
  -Name NAME       this engine's name, unique per machine (default: the hostname)
  -Version TAG     release to download, e.g. v2026.10.17 (else $env:ITER_VERSION, else latest)
  -Binary PATH     install this iter_engine.exe instead of downloading one
  -Yes             answer yes to every question; update while work runs
  -DryRun          print every action, change nothing
  ($env:ITER_RELEASE_URL: download the binary and SHA256SUMS from this base URL instead)

The steps of `install`: tools (Git for Windows, whose bash runs the shell
steps; the claude CLI; asks before winget) -> iter_data URL + engine name
(~\.iter\engine.conf) -> iter_engine.exe into ~\.iter\bin (the asset next to
this script, else the GitHub Release, sha256-checked) -> the embedding model
into ~\.iter\models -> the engine key (`iter_engine key new`: paste its public
key into the webui's Engine keys screen; the script waits until iter_data
accepts it) -> a laptop: no sleep on AC power (asks) -> the logon task
"iter engine" (runs ~\.iter\bin\iter-engine.ps1 start, a copy of this script)
-> account tokens and project checkouts.

Everything this machine keeps for iter is in ~\.iter: bin\iter_engine.exe, the
key (engine.key), the env file with account tokens (.env), engine.conf,
models\, engine.pid and the logs (engine.log, engine.err.log).
#>
param(
    [ValidateSet('install', 'status', 'update', 'accounts', 'projects', 'start', 'stop', 'logs', 'uninstall')]
    [string]$Mode = 'install',
    [string]$DataUrl = '',
    [string]$Name = '',
    [string]$Version = $env:ITER_VERSION,
    [string]$Binary = '',
    [switch]$Yes,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is many times slower with its progress bar
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { }
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$RepoSlug = 'Stephen-Hilton/iter'
$Asset = 'iter_engine-windows-x86_64.exe'   # also on ARM64 Windows, through its x64 emulation
$Here = $PSScriptRoot
$IterHome = Join-Path $HOME '.iter'
$BinDir = Join-Path $IterHome 'bin'
$Exe = Join-Path $BinDir 'iter_engine.exe'
$Launcher = Join-Path $BinDir 'iter-engine.ps1'
$Conf = Join-Path $IterHome 'engine.conf'
$EnvFile = Join-Path $IterHome '.env'
$KeyFile = Join-Path $IterHome 'engine.key'
$PidFile = Join-Path $IterHome 'engine.pid'
$Log = Join-Path $IterHome 'engine.log'
$ErrLog = Join-Path $IterHome 'engine.err.log'
$ModelDir = Join-Path $IterHome 'models\all-MiniLM-L6-v2'
$TaskName = 'iter engine'
$script:Engine = ''

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

function Load-Conf {
    if (-not $script:DataUrl) { $script:DataUrl = Get-Conf DATA_URL }
    if (-not $script:Engine) { $script:Engine = if ($Name) { $Name } else { Get-Conf ENGINE } }
    if (-not $script:Engine) { throw 'not installed yet: run install.cmd' }
}

# a session from the engine key (else the older fixed token), for this script's own API calls
function Get-Bearer {
    if ((Test-Path $KeyFile) -and (Test-Path $Exe)) {
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        try { $t = & $Exe key session --key-file $KeyFile --data-url $script:DataUrl 2>$null } finally { $ErrorActionPreference = $eap }
        if ($LASTEXITCODE -eq 0 -and $t) { return "$t".Trim() }
    }
    return Get-EnvValue 'ITER_ENGINE_TOKEN'
}

# GET an iter_data path as this engine (curl.exe: no PS 5.1 re-encoding); $null on failure. Read-only.
function Get-Api($path) {
    $ErrorActionPreference = 'Continue'
    $tok = Get-Bearer
    $out = & curl.exe -sS -f --max-time 15 -H "authorization: Bearer $tok" "$($script:DataUrl)$path" 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return ($out -join "`n") | ConvertFrom-Json
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

# ------------------------------------------------------------------ the engine process

function Get-EngineProcess {
    if (-not (Test-Path $PidFile)) { return $null }
    $p = Get-Process -Id ([int](Get-Content $PidFile -Raw).Trim()) -ErrorAction SilentlyContinue
    if ($p -and $p.ProcessName -eq 'iter_engine') { return $p }
    return $null
}

function Stop-Engine {
    $p = Get-EngineProcess
    if (-not $p) { return }
    if ($DryRun) { Dry "stop iter_engine (pid $($p.Id)) and its agent sessions (taskkill /T)"; return }
    Say "stopping iter_engine (pid $($p.Id))"
    # /T: the claude sessions and shells it started go with it
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    taskkill /T /F /PID $p.Id 2>&1 | Out-Null
    $ErrorActionPreference = $eap
    $p.WaitForExit(10000) | Out-Null
    Remove-Item $PidFile -ErrorAction SilentlyContinue
}

function Start-Engine {
    Load-Conf
    if (-not $DryRun -and -not (Test-Path $Exe)) { throw "$Exe is missing: run install.cmd" }
    if (-not (Test-Path $KeyFile) -and -not (Get-EnvValue 'ITER_ENGINE_TOKEN')) {
        if ($DryRun) { Dry "start iter_engine (once $KeyFile exists)"; return }
        throw "no engine key at ${KeyFile}: run install.cmd (or '$Exe key new', then add its public key in the webui's Engine keys screen)"
    }
    $running = Get-EngineProcess
    if ($running) { Ok "iter_engine already running (pid $($running.Id))"; return }
    $engineArgs = @('--data-url', $script:DataUrl, '--env-file', $EnvFile, '--key-file', $KeyFile, '--name', $script:Engine)
    if ($DryRun) { Dry "start (hidden) $Exe $($engineArgs -join ' ') > $Log 2> $ErrLog"; return }
    New-Item -ItemType Directory -Force $IterHome | Out-Null
    $p = Start-Process -FilePath $Exe -ArgumentList $engineArgs -WorkingDirectory $IterHome -WindowStyle Hidden `
        -RedirectStandardOutput $Log -RedirectStandardError $ErrLog -PassThru
    Set-Content $PidFile $p.Id
    Ok "iter_engine '$($script:Engine)' started (pid $($p.Id)) -> $($script:DataUrl); log $Log"
    $token = Get-Bearer
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep 1
        if ($p.HasExited) { Get-Content $ErrLog -Tail 20 -ErrorAction SilentlyContinue; throw "iter_engine exited ($($p.ExitCode))" }
        try {
            $e = Invoke-RestMethod "$($script:DataUrl)/api/engines/$($script:Engine)" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 3
            if ($e.state -eq 'Running') { Ok "engine checked in: state $($e.state)"; return }
        } catch { }
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

# install it as ~\.iter\bin\iter_engine.exe; true when the file changed. Windows
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
    if ($url -match '^http://' -and $url -notmatch '^http://(127\.0\.0\.1|localhost)') { Warn "$url is plain http: the engine's session token crosses the network unencrypted" }
    & curl.exe -sS -f --max-time 10 -o NUL "$url/health" 2>$null
    if ($LASTEXITCODE -eq 0) { Ok "$url answers" } else { Warn "no answer from $url/health (a proxy may hide it; the key check below tells)" }
    # hostname keeps its case; $env:COMPUTERNAME is upper-cased and would register a second engine
    $priorName = Get-Conf ENGINE
    $def = if ($priorName) { $priorName } else { hostname }
    $n = if ($Name) { $Name } else { Ask "engine name [$def]:" $def }
    if ($n -notmatch '^[A-Za-z0-9_.-]+$') { throw "engine name '$n': letters, digits, - _ . only" }
    Step "write $Conf (DATA_URL=$url, ENGINE=$n)" {
        New-Item -ItemType Directory -Force $IterHome | Out-Null
        [IO.File]::WriteAllLines($Conf, [string[]]@("DATA_URL=$url", "ENGINE=$n"))
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
}

# the embedding model (sentence-transformers/all-MiniLM-L6-v2, Apache-2.0,
# ~91 MB), checksum-verified; the engine finds ~\.iter\models beside its
# binary. Without it the engine downloads the model itself on first use.
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
    New-Item -ItemType Directory -Force $IterHome | Out-Null
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

# a logon task runs a copy of this script (~\.iter\bin\iter-engine.ps1 start),
# so the engine starts at logon without the download or checkout it came from
function Step-Service {
    Say '7/8 start at logon'
    if ($PSCommandPath -and ((Resolve-Path $PSCommandPath).Path -ne $Launcher)) {
        Step "copy this script to $Launcher" { New-Item -ItemType Directory -Force $BinDir | Out-Null; Copy-Item $PSCommandPath $Launcher -Force }
    } elseif (-not $PSCommandPath) { throw 'run this script from a file (not piped into Invoke-Expression): the logon task needs a copy of it' }
    $taskArgs = "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Launcher`" start"
    Step "register the logon task '$TaskName': powershell.exe $taskArgs" {
        $action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $taskArgs -WorkingDirectory $IterHome
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
        $set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -Hidden
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $set -Force `
            -Description "iter: start iter_engine '$($script:Engine)' against $($script:DataUrl)" | Out-Null
    }
    Ok "logon task '$TaskName'"
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
    $wasRunning = [bool](Get-EngineProcess)
    $changed = Install-Binary
    if ($PSCommandPath -and (Test-Path $Launcher) -and ((Resolve-Path $PSCommandPath).Path -ne $Launcher)) {
        Step "refresh $Launcher" { Copy-Item $PSCommandPath $Launcher -Force }
    }
    if ($changed -and $wasRunning) { Start-Engine }
}

function Do-Uninstall {
    Say 'uninstall the iter engine'
    Stop-Engine
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Step "unregister the logon task '$TaskName'" { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false }
        Ok "logon task '$TaskName' removed"
    }
    foreach ($f in $Exe, $Launcher) { if (Test-Path $f) { Step "remove $f" { Remove-Item $f -Force }; Ok "removed $f" } }
    Ok "kept $IterHome (key, tokens, model, logs) and the project checkouts; delete them by hand when this machine is done with iter"
    Ok "in the webui, remove the engine's key (Engine keys) and its serves edges"
}

if ($DryRun) { Write-Host 'dry run: every action is printed, nothing is changed' }
switch ($Mode) {
    'install' {
        Step-Tools; Step-Settings; Step-Binary; Step-Model; Step-Key; Step-Power; Step-Service
        Say '8/8 accounts and projects'
        Step-Accounts; Step-Projects
        if ($DryRun) { Write-Host "`nDry run done: nothing was changed."; break }
        Write-Host "`nDone. Engine '$($script:Engine)' starts at logon and checks in with $($script:DataUrl)."
        Write-Host 'Next, in the webui''s Settings tab: draw a serves edge from it to each project (topdir with forward'
        Write-Host 'slashes, e.g. C:/Users/<you>/dev/<project>), then run install.cmd projects here to clone them.'
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
