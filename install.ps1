<#
install.ps1 - make this Windows machine an iter engine. Not a full installer:
it checks the tools (offering winget for the missing ones), builds iter_engine
from this checkout, sets up the engine key and starts the engine at logon.
iter_data runs elsewhere; this machine only makes outbound connections.

From the folder that holds your code projects (PowerShell):

  cd ~\dev; git clone https://github.com/Stephen-Hilton/iter.git; .\iter\install.ps1

  .\iter\install.ps1             install / repair (safe to run again)
  .\iter\install.ps1 projects    clone each served project that is missing
  .\iter\install.ps1 status      the engine process, its key, the served projects
  .\iter\install.ps1 update      git pull, rebuild, restart (-Yes while work runs)

The steps: tools (Git for Windows, Rust + the MSVC build tools, the claude
CLI) -> iter_data URL and engine name (~\.iter\engine.conf) -> build
(deploy.ps1 build -> bin\windows-x86_64\iter_engine.exe) -> embedding model
(models\ in this repo) -> engine key (`iter_engine key new`: the private key stays
in ~\.iter\engine.key; paste its public key in the webui's Engine keys screen, the
script waits until iter_data accepts it)
laptop: no sleep on AC power -> a logon task that starts the engine
(deploy.ps1 start) -> project checkouts.

Projects live beside this repo: give the engine's `serves` edge (webui,
Settings tab) the topdir ~/dev/<project>, e.g. ~/dev/rendfall.
#>
param(
    [ValidateSet('install', 'projects', 'status', 'update')]
    [string]$Mode = 'install',
    [switch]$Yes
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8

$Repo = $PSScriptRoot
# everything this machine keeps for iter (~\.iter5 became ~\.iter, 2026-10-11:
# deploy.ps1 moves an older machine's folder over when no engine runs from it)
$IterHome = Join-Path $HOME '.iter'
$Conf = Join-Path $IterHome 'engine.conf'
$EnvFile = Join-Path $IterHome '.env'
$KeyFile = Join-Path $IterHome 'engine.key'
$Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$Exe = Join-Path $Repo "bin\windows-$Arch\iter_engine.exe"
$Deploy = Join-Path $Repo 'deploy.ps1'
$TaskName = 'iter engine'

function Say($m) { Write-Host "`n==> $m" }
function Ok($m) { Write-Host "    ok  $m" -ForegroundColor Green }
function Warn($m) { Write-Host "    !!  $m" -ForegroundColor Yellow }
function Ask($q, $default) { $a = Read-Host "    $q"; if ([string]::IsNullOrWhiteSpace($a)) { return $default } return $a.Trim() }
function YesNo($q) { return (Ask "$q [Y/n]" 'y') -notmatch '^[nN]' }

# Native tools write progress to stderr; PowerShell 5.1 must not treat that as an error.
function Invoke-Native([string]$what, [scriptblock]$cmd) {
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { & $cmd 2>&1 | ForEach-Object { "    $_" } | Out-Host } finally { $ErrorActionPreference = $eap }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    foreach ($d in (Join-Path $HOME '.cargo\bin'), (Join-Path $HOME '.local\bin')) { if ((Test-Path $d) -and ($env:Path -notlike "*$d*")) { $env:Path = "$d;$env:Path" } }
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
    $l = Get-Content $EnvFile | Where-Object { $_ -match "^\s*$key\s*=" } | Select-Object -Last 1
    if ($l) { return ($l -replace "^\s*$key\s*=\s*", '').Trim().Trim('"', "'") } return ''
}

# GET an iter_data path as this engine (a session from its key; curl.exe: no PS 5.1 re-encoding); $null on failure
function Get-Api($path) {
    $ErrorActionPreference = 'Continue'  # curl's stderr is not an error here
    $tok = if (Test-Path $KeyFile) { "$(& $Exe key session --key-file $KeyFile --data-url $script:DataUrl 2>$null)".Trim() } else { Get-EnvValue 'ITER_ENGINE_TOKEN' }
    $out = & curl.exe -sS -f --max-time 15 -H "authorization: Bearer $tok" "$script:DataUrl$path" 2>$null
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

# ------------------------------------------------------------------ steps

function Step-Tools {
    Say '1/7 tools'
    Refresh-Path
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Winget-Install 'Git.Git' 'Git for Windows' }
    Ok "git $((git --version) -replace 'git version ', '')"
    # Windows-native: shell steps run in Git for Windows' bash, never WSL's
    # (a bare `bash` on Windows can be System32\bash.exe, the WSL launcher)
    $script:Bash = Find-GitBash
    if (-not $script:Bash) { throw "Git for Windows' bash.exe was not found beside git.exe: reinstall Git for Windows (winget install Git.Git)" }
    Ok "Git Bash $($script:Bash) (shell steps; WSL is never used)"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { Winget-Install 'Rustlang.Rustup' 'Rust (rustup)' }
        Invoke-Native 'rustup default' { rustup default stable-msvc }
    }
    Ok "cargo $(((cargo --version) -split ' ')[1])"
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vc = if (Test-Path $vswhere) { & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath } else { $null }
    if (-not $vc) {
        if (-not (YesNo 'Rust on Windows needs the MSVC build tools (several GB). Install them with winget?')) { throw 'the MSVC build tools are needed to build iter_engine' }
        Invoke-Native 'winget install BuildTools' { winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements --override '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended' }
    }
    Ok 'MSVC build tools'
    if (Get-Command claude -ErrorAction SilentlyContinue) { Ok "claude $(cmd /c 'claude --version 2>nul' | Select-Object -First 1)" }
    elseif (YesNo "install the claude CLI (Anthropic's installer)?") {
        Invoke-Expression (Invoke-RestMethod 'https://claude.ai/install.ps1'); Refresh-Path
        if (Get-Command claude -ErrorAction SilentlyContinue) { Ok 'claude installed' } else { Warn 'claude is not on PATH yet: open a new PowerShell' }
    } else { Warn 'agents cannot run until the claude CLI is installed' }
    if (-not (Get-Command pdftoppm -ErrorAction SilentlyContinue)) { Warn 'pdftoppm (poppler) not found: GraphRAG skips PDF pages until it is on PATH' }
}

function Step-Settings {
    $ErrorActionPreference = 'Continue'
    Say '2/7 iter_data and this engine''s name'
    $url = (Ask "iter_data URL [$(if (Get-Conf DATA_URL) { Get-Conf DATA_URL } else { 'https://iter.example.com' })]:" (Get-Conf DATA_URL)).TrimEnd('/')
    if (-not $url) { throw 'the iter_data URL is required' }
    if ($url -match '^http://' -and $url -notmatch '^http://(127\.0\.0\.1|localhost)') { Warn "$url is plain http: the engine's session token crosses the network unencrypted" }
    & curl.exe -sS -f --max-time 10 -o NUL "$url/health" 2>$null
    if ($LASTEXITCODE -eq 0) { Ok "$url answers" } else { Warn "no answer from $url/health (a proxy may hide it; the key check below tells)" }
    $prior = Get-Conf ENGINE
    $name = Ask "engine name [$(if ($prior) { $prior } else { hostname })]:" $(if ($prior) { $prior } else { hostname })
    if ($name -notmatch '^[A-Za-z0-9_.-]+$') { throw "engine name '$name': letters, digits, - _ . only" }
    New-Item -ItemType Directory -Force $IterHome | Out-Null
    [IO.File]::WriteAllLines($Conf, [string[]]@("DATA_URL=$url", "ENGINE=$name"))
    $script:DataUrl = $url; $script:Engine = $name
    Ok "engine '$name' -> $url (saved in $Conf)"
}

function Load-Conf {
    $script:DataUrl = Get-Conf DATA_URL; $script:Engine = Get-Conf ENGINE
    if (-not $script:Engine) { throw 'not installed yet: .\iter\install.ps1' }
}

function Step-Build {
    Say '3/7 iter_engine'
    & $Deploy build
    Ok $Exe
    Say '4/7 embedding model'
    if (-not $script:Bash) { $script:Bash = Find-GitBash }
    Invoke-Native 'fetch_model.sh' { & $script:Bash (Join-Path $Repo 'tools/fetch_model.sh') }
}

function Step-Key {
    $ErrorActionPreference = 'Continue'
    Say '5/7 engine key'
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
        switch (Ask 'press Enter once it is added (n = a new key, s = skip):' '') {
            { $_ -in 'n', 'N' } {
                Remove-Item $KeyFile -Force
                & $Exe key new --key-file $KeyFile --name $script:Engine | ForEach-Object { "    $_" }; $first = $true
            }
            { $_ -in 's', 'S' } { Warn 'skipped: iter_data refuses this engine until its public key is added'; return }
        }
    }
}

function Step-Power {
    Say '6/7 power'
    if (-not (Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue)) { Ok 'not a laptop'; return }
    if (YesNo 'this laptop runs agents unattended: never sleep on AC power, lid closed or not?') {
        powercfg /change standby-timeout-ac 0
        powercfg /change hibernate-timeout-ac 0
        powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 0
        powercfg /setactive SCHEME_CURRENT
        Ok 'no sleep or hibernate on AC; closing the lid does nothing on AC'
    } else { Warn 'a sleeping laptop stops its agents mid-run' }
}

function Step-Service {
    Say '7/7 start at logon'
    $taskArgs = "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Deploy`" start -DataUrl `"$($script:DataUrl)`" -EnvFile `"$EnvFile`" -Name `"$($script:Engine)`""
    $action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $taskArgs -WorkingDirectory $Repo
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
    $set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -Hidden
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $set -Force `
        -Description "iter: start iter_engine '$($script:Engine)' against $($script:DataUrl)" | Out-Null
    Ok "logon task '$TaskName'"
    & $Deploy start -DataUrl $script:DataUrl -EnvFile $EnvFile -Name $script:Engine
}

function Step-Projects {
    Say 'project checkouts'
    $a = Get-Api "/api/engines/$($script:Engine)/assignments"
    if (-not $a -or -not $a.projects) {
        Ok "no projects served yet: in the webui's Settings tab draw a serves edge from '$($script:Engine)' to each project, topdir ~/dev/<project>; then .\iter\install.ps1 projects"
        return
    }
    foreach ($p in $a.projects) {
        if (-not $p.topdir) { Warn "$($p.project): its serves edge has no topdir (set ~/dev/$($p.project) in the Settings tab)"; continue }
        $dir = Expand-Topdir $p.topdir
        if (Test-Path (Join-Path $dir '.git')) { Ok "$($p.project): $dir"; continue }
        if ((Test-Path $dir) -and (Get-ChildItem $dir -Force | Select-Object -First 1)) { Warn "$($p.project): $dir exists but is not a git checkout; left alone"; continue }
        $rec = Get-Api "/api/projects/$($p.project)"
        $url = if ($rec -and $rec.gitrepo) { $rec.gitrepo } else { '' }
        $url = Ask "$($p.project): git URL to clone into $dir [$(if ($url) { $url } else { 'none' })]:" $url
        if (-not $url) { Warn "$($p.project): no checkout (the engine skips it until $dir exists)"; continue }
        & git clone -q $url $dir
        if ($LASTEXITCODE -eq 0) { Ok "$($p.project): cloned into $dir" } else { Warn "$($p.project): clone failed (credentials? try: gh auth login)" }
    }
}

function Show-Status {
    Load-Conf
    Write-Host "engine   $($script:Engine) -> $($script:DataUrl)"
    & $Deploy status
    if (Test-Path $Exe) { & $Exe key status --key-file $KeyFile --data-url $script:DataUrl }
    $a = Get-Api "/api/engines/$($script:Engine)/assignments"
    if ($a -and $a.projects) { $a.projects | ForEach-Object { Write-Host "project  $($_.project)  $($_.topdir)  $($_.state)" } } else { Write-Host 'project  (none served)' }
}

function Do-Update {
    Load-Conf
    $e = Get-Api "/api/engines/$($script:Engine)"
    if ($e -and $e.running -gt 0 -and -not $Yes) { throw "engine '$($script:Engine)' is running $($e.running) item(s): set its projects Draining and wait, or rerun with -Yes (stops them)" }
    Say "updating $Repo"
    Invoke-Native 'git pull' { git -C $Repo pull --ff-only -q }
    & $Deploy build
    & $Deploy start -DataUrl $script:DataUrl -EnvFile $EnvFile -Name $script:Engine
}

switch ($Mode) {
    'install' {
        Step-Tools; Step-Settings; Step-Build; Step-Key; Step-Power; Step-Service; Step-Projects
        Write-Host "`nDone. Engine '$($script:Engine)' starts at logon and checks in with $($script:DataUrl)."
        Write-Host 'Next, in the webui''s Settings tab: draw a serves edge from it to each project (topdir ~/dev/<project>),'
        Write-Host 'then run .\iter\install.ps1 projects here to clone them.'
    }
    'projects' { Load-Conf; Step-Projects }
    'status' { Show-Status }
    'update' { Do-Update }
}
