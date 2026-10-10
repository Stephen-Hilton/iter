<#
Deploy iter5 on Windows (native PowerShell; deploy.sh is the Linux/macOS twin).

  .\deploy.ps1 engine     build iter_engine for Windows, then (re)start it against iter_data
  .\deploy.ps1 build      build iter_engine only (-> bin\windows-x86_64\)
  .\deploy.ps1 start | stop | status    the local engine process
  .\deploy.ps1 startup    start the WSL distro (its systemd runs ArangoDB, iter_data
                          and its engine), wait for iter_data, start the Windows engine
  .\deploy.ps1 autostart  register `startup` as a logon task (and WSL memory reclaim)
  .\deploy.ps1 restart-wsl  restart the WSL VM (to apply ~\.wslconfig or engine unit
                          changes) without breaking Docker Desktop: refuses while any
                          engine is running work (-Force overrides), stops Docker
                          Desktop first, `wsl --shutdown`, restarts the distro and its
                          keepalive, then starts Docker Desktop again

The server (ArangoDB + iter_data on :8400) runs in a WSL2 distro, installed
there with linux/install.sh; `docker` only says so now.

Engine settings (defaults in brackets):
  -DataUrl  [$env:ITER_DATA_URL, else http://127.0.0.1:8400]
  -EnvFile  [~\.iter5\.env]   ITER_ENGINE_TOKEN + each account's token variable
  -Name     [the hostname]
  -Distro   [Debian]   the WSL distro running the server (startup, autostart)
The engine's pid and log live in ~\.iter5\ (engine.pid, engine.log, engine.err.log).
Needs: Rust (rustup, MSVC toolchain), Git for Windows (bash, for shell steps), WSL2.
#>
param(
    [ValidateSet('docker', 'engine', 'build', 'start', 'stop', 'status', 'startup', 'autostart', 'restart-wsl')]
    [string]$Mode = 'engine',
    [string]$Distro = 'Debian',
    [switch]$Hold,
    [switch]$Force,
    [string]$DataUrl = $(if ($env:ITER_DATA_URL) { $env:ITER_DATA_URL } else { 'http://127.0.0.1:8400' }),
    [string]$EnvFile = (Join-Path $HOME '.iter5\.env'),
    # hostname keeps its case (TheBEAST); $env:COMPUTERNAME is upper-cased
    # and would register a second engine
    [string]$Name = (hostname)
)
$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
$Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$Bin = Join-Path $Root "bin\windows-$Arch"
$Exe = Join-Path $Bin 'iter_engine.exe'
$Home5 = Join-Path $HOME '.iter5'
$PidFile = Join-Path $Home5 'engine.pid'
$Log = Join-Path $Home5 'engine.log'
$ErrLog = Join-Path $Home5 'engine.err.log'
$Port = if ($env:ITER_PORT) { $env:ITER_PORT } else { '8400' }

function Say($m) { Write-Host "[deploy] $m" }

# Run a native command; its stderr (cargo/docker progress) is output, not an
# error. Windows PowerShell 5.1 turns redirected stderr into terminating
# errors under ErrorActionPreference=Stop. Throws on a non-zero exit.
function Invoke-Native([string]$what, [scriptblock]$cmd) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $cmd 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" }
        } | Out-Host
    } finally { $ErrorActionPreference = $eap }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

function Find-Cargo {
    $c = Get-Command cargo -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $c = Join-Path $HOME '.cargo\bin\cargo.exe'
    if (Test-Path $c) { return $c }
    throw 'cargo not found: install Rust with `winget install Rustlang.Rustup`'
}

# a single KEY=value from a .env file, without running it
function Get-EnvValue($file, $key) {
    if (-not (Test-Path $file)) { return '' }
    $line = Get-Content $file | Where-Object { $_ -match "^\s*(export\s+)?$key\s*=" } | Select-Object -Last 1
    if (-not $line) { return '' }
    return ($line -replace "^\s*(export\s+)?$key\s*=\s*", '').Trim().Trim('"', "'")
}

function Get-EngineProcess {
    if (-not (Test-Path $PidFile)) { return $null }
    $p = Get-Process -Id ([int](Get-Content $PidFile -Raw).Trim()) -ErrorAction SilentlyContinue
    if ($p -and $p.ProcessName -eq 'iter_engine') { return $p }
    return $null
}

function Build-Engine {
    $cargo = Find-Cargo
    Say "building iter_engine (release, windows-$Arch)"
    Push-Location $Root
    try {
        Invoke-Native 'cargo build' { & $cargo build --release -p iter_engine }
    } finally { Pop-Location }
    Stop-Engine   # Windows will not overwrite a running .exe
    New-Item -ItemType Directory -Force $Bin | Out-Null
    Copy-Item (Join-Path $Root 'target\release\iter_engine.exe') $Exe -Force
    Say "binary -> $Exe"
}

function Stop-Engine {
    $p = Get-EngineProcess
    if ($p) {
        Say "stopping iter_engine (pid $($p.Id))"
        # /T: the claude sessions and shells it started go with it
        $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        taskkill /T /F /PID $p.Id 2>&1 | Out-Null
        $ErrorActionPreference = $eap
        $p.WaitForExit(10000) | Out-Null
    }
    Remove-Item $PidFile -ErrorAction SilentlyContinue
}

function Start-Engine {
    if (-not (Test-Path $Exe)) { throw "$Exe is missing: run .\deploy.ps1 build" }
    if (-not (Test-Path $EnvFile)) { throw "$EnvFile is missing (needs ITER_ENGINE_TOKEN and the account tokens)" }
    if (-not (Get-EnvValue $EnvFile 'ITER_ENGINE_TOKEN')) { throw "ITER_ENGINE_TOKEN is not set in $EnvFile" }
    $running = Get-EngineProcess
    if ($running) { Say "iter_engine already running (pid $($running.Id))"; return }
    New-Item -ItemType Directory -Force $Home5 | Out-Null
    $engineArgs = @('--data-url', $DataUrl, '--env-file', $EnvFile, '--name', $Name)
    $p = Start-Process -FilePath $Exe -ArgumentList $engineArgs -WorkingDirectory $Home5 -WindowStyle Hidden `
        -RedirectStandardOutput $Log -RedirectStandardError $ErrLog -PassThru
    Set-Content $PidFile $p.Id
    Say "iter_engine '$Name' started (pid $($p.Id)) -> $DataUrl; log $Log"
    # wait for its first heartbeat
    $token = Get-EnvValue $EnvFile 'ITER_ENGINE_TOKEN'
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep 1
        if ($p.HasExited) { Get-Content $ErrLog -Tail 20 -ErrorAction SilentlyContinue; throw "iter_engine exited ($($p.ExitCode))" }
        try {
            $e = Invoke-RestMethod "$DataUrl/api/engines/$Name" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 3
            if ($e.state -eq 'Running') { Say "engine checked in: state $($e.state), last_seen $($e.last_seen)"; return }
        } catch { }
    }
    Say "WARNING: no Running check-in after 30s; see $Log and $ErrLog"
}

function Show-Status {
    $p = Get-EngineProcess
    if ($p) { Say "iter_engine running (pid $($p.Id), started $($p.StartTime))" } else { Say 'iter_engine not running' }
    if (Test-Path $Log) { Get-Content $Log -Tail 5 }
}

function Wait-Health([int]$seconds = 120) {
    for ($i = 0; $i -lt $seconds; $i++) {
        try {
            $h = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2
            if ($h.ok) { Say "iter_data up: http://127.0.0.1:$Port  $($h | ConvertTo-Json -Compress)"; return }
        } catch { }
        Start-Sleep 1
    }
    throw "iter_data did not answer on :$Port within ${seconds}s (in WSL: linux/install.sh status)"
}

# The WSL VM keeps freed page cache (a cargo build leaves ~10 GB) counted
# against Windows unless WSL reclaims it; make sure ~\.wslconfig asks for
# that. It takes effect on the next `wsl --shutdown`.
function Confirm-WslMemoryReclaim {
    $cfg = Join-Path $env:USERPROFILE '.wslconfig'
    $lines = @(if (Test-Path $cfg) { Get-Content $cfg })
    if ($lines | Where-Object { $_ -match '^\s*autoMemoryReclaim\s*=' }) { return }
    $i = [array]::FindIndex([string[]]$lines, [Predicate[string]]{ param($l) $l -match '^\s*\[experimental\]\s*$' })
    $set = 'autoMemoryReclaim=gradual'
    if ($i -ge 0) {
        $lines = @($lines[0..$i]) + $set + @(if ($i + 1 -lt $lines.Count) { $lines[($i + 1)..($lines.Count - 1)] })
    } else {
        $lines = $lines + @(if ($lines.Count) { '' }) + '[experimental]' + $set
    }
    [IO.File]::WriteAllLines($cfg, [string[]]$lines)
    Say "added $set to $cfg; run 'wsl --shutdown' (restarts Docker's VM) for it to apply"
}

# On Windows the server (ArangoDB + iter_data) runs natively in a WSL2
# distro as systemd services (linux/install.sh), not in a container: the
# database stays off the slow Windows-drive mount and Linux-heavy projects get
# their own engine there. Its port 8400 reaches Windows as 127.0.0.1:8400.
function Show-DockerMoved {
    Say 'the docker deploy is retired on Windows: the server runs in a WSL2 distro instead.'
    Say "  in the distro:  linux/install.sh server   (then: install.sh engine --name <name>)"
    Say "  on Windows:     .\deploy.ps1 autostart          (start it all at logon)"
    Say '(./deploy.sh docker remains for Linux and macOS hosts)'
}

# A WSL distro stops when no Windows process holds it; one hidden
# `wsl.exe -d <distro> sleep infinity` keeps it (and its systemd services) up.
function Get-WslKeepAlive {
    Get-CimInstance Win32_Process -Filter "Name='wsl.exe'" |
        Where-Object { $_.CommandLine -match "-d\s+$Distro\s.*sleep infinity" }
}

# Bring everything up after a reboot: the distro (whose systemd starts
# ArangoDB, iter_data and its engine), then this machine's Windows engine.
# -Hold keeps the keepalive in the foreground (for the logon task).
function Start-All {
    $keep = Get-WslKeepAlive | Select-Object -First 1
    if (-not $keep) {
        Say "starting WSL distro $Distro"
        Start-Process -FilePath "$env:WINDIR\System32\wsl.exe" -ArgumentList "-d $Distro --exec /bin/sleep infinity" -WindowStyle Hidden | Out-Null
        Start-Sleep 2
        $keep = Get-WslKeepAlive | Select-Object -First 1
    }
    Wait-Health 300
    Start-Engine
    if ($Hold -and $keep) { Say "holding WSL distro $Distro (pid $($keep.ProcessId))"; Wait-Process -Id $keep.ProcessId }
}

# Run Start-All at every logon (Task Scheduler, current user, no admin).
function Register-AutoStart {
    Confirm-WslMemoryReclaim
    $task = 'iter startup'
    $taskArgs = "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$PSCommandPath`" startup -Hold -Distro $Distro"
    $action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $taskArgs -WorkingDirectory $Root
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -Hidden
    Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Settings $settings -Force `
        -Description "iter5: start WSL distro $Distro (ArangoDB, iter_data, its engine) and this machine's Windows engine" | Out-Null
    Say "registered logon task '$task' (WSL distro $Distro, then iter_engine '$Name')"
}

# Docker Desktop runs its own WSL distro and a proxy inside $Distro; a bare
# `wsl --shutdown` under it leaves its shared sockets gone and pops up "WSL
# integration with distro ... unexpectedly stopped" every ~30 s (restarting
# the integration does not help). So: stop it fully first, start it after.
$DockerDesktopExe = Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'
$DockerProcs = 'Docker Desktop', 'com.docker.backend', 'com.docker.build', 'docker-agent'

function Stop-DockerDesktop {
    $procs = Get-Process $DockerProcs -ErrorAction SilentlyContinue
    if (-not $procs) { return $false }
    Say 'stopping Docker Desktop'
    $procs | Stop-Process -Force -ErrorAction SilentlyContinue
    for ($i = 0; $i -lt 30 -and (Get-Process $DockerProcs -ErrorAction SilentlyContinue); $i++) { Start-Sleep 1 }
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    wsl.exe --terminate docker-desktop 2>&1 | Out-Null
    $ErrorActionPreference = $eap
    return $true
}

function Start-DockerDesktop {
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try {
        # sockets left by the earlier Docker run would be taken for live ones
        wsl.exe -d $Distro -u root -e rm -rf /mnt/wsl/docker-desktop /mnt/wsl/docker-desktop-bind-mounts 2>&1 | Out-Null
        Say 'starting Docker Desktop'
        Start-Process -FilePath $DockerDesktopExe | Out-Null
        for ($i = 0; $i -lt 60; $i++) {
            Start-Sleep 3
            $v = wsl.exe -d $Distro -e docker version --format '{{.Server.Version}}' 2>$null
            if ($LASTEXITCODE -eq 0 -and $v) { Say "docker $v reachable in $Distro"; return }
        }
        Say "WARNING: docker not reachable in $Distro after 180s; check Docker Desktop"
    } finally { $ErrorActionPreference = $eap }
}

# "<engine>: N running" for every engine with work in flight (any OS): the
# shutdown takes iter_data down and kills the WSL engine's agent sessions.
function Get-RunningWork {
    $token = Get-EnvValue $EnvFile 'ITER_ENGINE_TOKEN'
    $engines = Invoke-RestMethod "$DataUrl/api/engines" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 5
    @($engines | Where-Object { $_.running -gt 0 } | ForEach-Object { "$($_.name): $($_.running) running" })
}

# Restart the WSL VM (applies ~\.wslconfig and systemd unit changes) without
# leaving Docker Desktop's integration broken or the distro unheld.
function Restart-Wsl {
    $busy = @()
    try { $busy = Get-RunningWork } catch {
        if (-not $Force) { throw "cannot read engine status from $DataUrl ($_); -Force restarts anyway" }
    }
    if ($busy.Count -and -not $Force) {
        throw "work is running ($($busy -join '; ')): set those projects Draining and wait, or -Force (kills the sessions)"
    }
    $hadDocker = Stop-DockerDesktop
    Say 'wsl --shutdown'
    Invoke-Native 'wsl --shutdown' { wsl.exe --shutdown }
    Start-Sleep 3
    Start-All   # distro + keepalive, iter_data health, this machine's engine
    if ($hadDocker) { Start-DockerDesktop }
}

switch ($Mode) {
    'docker'    { Show-DockerMoved; exit 1 }
    'build'     { Build-Engine }
    'engine'    { Build-Engine; Start-Engine }
    'start'     { Start-Engine }
    'stop'      { Stop-Engine; Say 'stopped' }
    'status'    { Show-Status }
    'startup'   { Start-All }
    'autostart' { Register-AutoStart }
    'restart-wsl' { Restart-Wsl }
}
