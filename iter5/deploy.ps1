<#
Deploy iter5 on Windows (native PowerShell; deploy.sh is the Linux/macOS twin).

  .\deploy.ps1 docker   build + start the all-in-one container (ArangoDB CE + iter_data) on :8400
  .\deploy.ps1 engine   build iter_engine for Windows, then (re)start it against iter_data
  .\deploy.ps1 build    build iter_engine only (-> bin\windows-x86_64\)
  .\deploy.ps1 start | stop | status    the local engine process

Engine settings (defaults in brackets):
  -DataUrl  [$env:ITER_DATA_URL, else http://127.0.0.1:8400]
  -EnvFile  [~\.iter5\.env]   ITER_ENGINE_TOKEN + each account's token variable
  -Name     [the hostname]
The engine's pid and log live in ~\.iter5\ (engine.pid, engine.log, engine.err.log).
Needs: Rust (rustup, MSVC toolchain), Git for Windows (bash, for shell steps), Docker Desktop.
#>
param(
    [ValidateSet('docker', 'engine', 'build', 'start', 'stop', 'status')]
    [string]$Mode = 'engine',
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

# Git for Windows' bash; never System32\bash.exe, which is WSL
function Find-GitBash {
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) {
        $gitRoot = Split-Path (Split-Path $git.Source)
        foreach ($b in @((Join-Path $gitRoot 'bin\bash.exe'), (Join-Path (Split-Path $gitRoot) 'bin\bash.exe'))) {
            if (Test-Path $b) { return $b }
        }
    }
    $b = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
    if (Test-Path $b) { return $b }
    throw 'Git for Windows bash not found (install Git for Windows)'
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

function Wait-Health {
    for ($i = 0; $i -lt 120; $i++) {
        try {
            $h = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2
            if ($h.ok) { Say "iter_data up: http://127.0.0.1:$Port  $($h | ConvertTo-Json -Compress)"; return }
        } catch { }
        Start-Sleep 1
    }
    docker logs --tail 40 iter5
    throw 'iter_data did not come up'
}

function Deploy-Docker {
    $repoEnv = if ($env:ITER_ENV_FILE) { $env:ITER_ENV_FILE } else { Join-Path $Root '..\.env' }
    $run = Join-Path $Root 'run'
    New-Item -ItemType Directory -Force $run | Out-Null
    $lines = @(
        "ITER_ADMIN_PASSWORD=$(Get-EnvValue $repoEnv 'ITER_ADMIN_PASSWORD')",
        "ITER_JWT_SECRET=$(Get-EnvValue $repoEnv 'ITER_JWT_SECRET')"
    )
    # no BOM: docker compose reads the first key name literally
    [IO.File]::WriteAllLines((Join-Path $run 'docker.env'), $lines)
    $model = Join-Path $Root 'models\all-MiniLM-L6-v2\model.safetensors'
    if (-not (Test-Path $model)) {
        Say 'fetching the embedding model'
        $bash = Find-GitBash
        Invoke-Native 'fetch_model.sh' { & $bash (Join-Path $Root 'tools/fetch_model.sh') }
    }
    Say 'building + starting the iter5 container'
    $env:ARANGO_ROOT_PASSWORD = if ($env:ARANGO_ROOT_PASSWORD) { $env:ARANGO_ROOT_PASSWORD } else { 'iter4dev' }
    $env:ITER_PORT = $Port
    $compose = Join-Path $Root 'docker\compose.yml'
    Invoke-Native 'docker compose' { docker compose -f $compose up -d --build }
    Wait-Health
    Say "webui: http://127.0.0.1:$Port/  (login: admin / ITER_ADMIN_PASSWORD)"
}

switch ($Mode) {
    'docker' { Deploy-Docker }
    'build'  { Build-Engine }
    'engine' { Build-Engine; Start-Engine }
    'start'  { Start-Engine }
    'stop'   { Stop-Engine; Say 'stopped' }
    'status' { Show-Status }
}
