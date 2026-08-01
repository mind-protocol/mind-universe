<#
.SYNOPSIS
    Boots the `mind-mcp` arrive/sense adapter, bridges its stdio JSON-RPC to
    streamable HTTP, and publishes it through a reserved ngrok domain.

.DESCRIPTION
    mind-mcp speaks JSON-RPC 2.0 over stdio (mcp/README.md). ngrok exposes HTTP,
    so this script inserts `supergateway` as the stdio -> streamable-HTTP bridge:

        mind-mcp (stdio)  ->  supergateway (:$Port/$Path)  ->  ngrok ($Domain)

    Both children are supervised: if either dies it is restarted (bounded
    backoff). Ctrl+C tears the whole tree down cleanly.

    The Universe is mounted read/act from the live ontology-registry store
    (UNIVERSE_STORE) + minimal genesis (UNIVERSE_GENESIS); see mcp/src/world.rs.
    An unmounted boot is non-fatal: the transport still serves, honestly
    reporting `uncertainty: unknown`.

.EXAMPLE
    pwsh -File scripts/serve-mcp-ngrok.ps1

.EXAMPLE
    pwsh -File scripts/serve-mcp-ngrok.ps1 -Release -Port 8000
#>
[CmdletBinding()]
param(
    [string]$Domain  = 'trusted-magpie-social.ngrok-free.app',
    [int]   $Port    = 8000,
    # Transport to expose. ChatGPT connectors want streamable HTTP (/mcp,
    # default, served by our own scripts/http-bridge.mjs). 'sse' (/sse) is
    # available via supergateway for legacy SSE clients.
    [ValidateSet('sse','streamableHttp')]
    [string]$Transport = 'streamableHttp',
    [string]$Store   = 'artifacts/ontology-registry/current/store',
    [string]$Genesis = 'fixtures/genesis/minimal-genesis.json',
    [switch]$Release,
    # Serve the binary that is already on disk instead of rebuilding it. Needed
    # when another mind-mcp process (e.g. a live Claude Code MCP session) holds
    # the exe open: cargo cannot replace a locked file and the build fails with
    # os error 5. The trade-off is explicit and yours: the served binary may be
    # older than the working tree, so what it reports is the world as of THAT
    # build. The script prints the binary's timestamp so the staleness is
    # visible rather than assumed.
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

# --- resolve paths relative to the repo root (this script lives in scripts/) ---
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Resolve-RepoPath([string]$p) {
    if ([System.IO.Path]::IsPathRooted($p)) { return $p }
    return (Join-Path $RepoRoot $p)
}

$StorePath   = Resolve-RepoPath $Store
$GenesisPath = Resolve-RepoPath $Genesis
$Manifest    = Join-Path $RepoRoot 'mcp\Cargo.toml'
$Profile     = if ($Release) { 'release' } else { 'debug' }
$BinPath     = Join-Path $RepoRoot "mcp\target\$Profile\mind-mcp.exe"

# Fallback build tree. The shared `mcp\target` exe is routinely held open by
# another long-lived mind-mcp (a Claude Code MCP session spawns one and keeps it
# for the whole session), and cargo cannot replace a locked file: it aborts with
# "Access is denied. (os error 5)". Rather than kill someone else's live server,
# we rebuild into a private target dir that nothing else holds. Same source, same
# profile — only the output path differs.
$ServeTargetDir = Join-Path $RepoRoot 'mcp\target\serve'
$ServeBinPath   = Join-Path $ServeTargetDir "$Profile\mind-mcp.exe"

# The client-facing path depends on the transport: SSE clients connect to /sse
# (and POST replies to /message), streamable-HTTP clients use /mcp.
$Path = if ($Transport -eq 'sse') { '/sse' } else { '/mcp' }

Write-Host "mind-universe :: serve MCP over ngrok" -ForegroundColor Cyan
Write-Host "  repo      $RepoRoot"
Write-Host "  store     $StorePath"
Write-Host "  genesis   $GenesisPath"
Write-Host "  transport $Transport"
Write-Host "  bridge    http://127.0.0.1:$Port$Path  ($Profile build)"
Write-Host "  public    https://$Domain$Path"
Write-Host ""

# --- prerequisite checks -----------------------------------------------------
function Require-Cmd([string]$name, [string]$hint) {
    $c = Get-Command $name -ErrorAction SilentlyContinue
    if (-not $c) { Write-Error "Missing '$name'. $hint"; exit 1 }
    return $c
}
Require-Cmd 'cargo' 'Install the Rust toolchain (https://rustup.rs).' | Out-Null
Require-Cmd 'npx'   'Install Node.js (npx ships with it) so supergateway can run.' | Out-Null
# ngrok is not auth-gated here: no authtoken prerequisite. If it is absent we
# only warn; the supervisor still starts the bridge and will surface any ngrok
# error at runtime.
if (-not (Get-Command 'ngrok' -ErrorAction SilentlyContinue)) {
    Write-Warning "'ngrok' not found on PATH -> only the local bridge (127.0.0.1:$Port$Path) will serve; the public tunnel will keep retrying."
}

if (-not (Test-Path $StorePath))   { Write-Warning "Store not found at $StorePath -> MCP will serve UNMOUNTED (honest 'unknown')." }
if (-not (Test-Path $GenesisPath)) { Write-Warning "Genesis not found at $GenesisPath -> MCP will serve UNMOUNTED (honest 'unknown')." }

# --- build the adapter -------------------------------------------------------
if ($SkipBuild) {
    if (-not (Test-Path $BinPath)) { Write-Error "-SkipBuild given but no binary at $BinPath. Run once without it."; exit 1 }
    $built = (Get-Item $BinPath).LastWriteTime
    Write-Warning "[build] SKIPPED -> serving the binary built $built. It may be older than the working tree."
} else {
    # A held exe is detectable before cargo tries to overwrite it: opening the
    # file for write throws while any process has it mapped. Cheaper and clearer
    # than parsing cargo's error text.
    $locked = $false
    if (Test-Path $BinPath) {
        try { $fs = [System.IO.File]::Open($BinPath, 'Open', 'Write', 'None'); $fs.Close() }
        catch { $locked = $true }
    }

    $buildArgs = @('build', '--manifest-path', $Manifest)
    if ($Release) { $buildArgs += '--release' }
    if ($locked) {
        Write-Warning "[build] $BinPath is held by another process -> building into $ServeTargetDir instead (nobody else's server gets killed)."
        $buildArgs += @('--target-dir', $ServeTargetDir)
        $BinPath = $ServeBinPath
    }

    Write-Host "[build] cargo build ($Profile) mind-mcp -> $BinPath ..." -ForegroundColor Yellow
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Error "cargo build failed ($LASTEXITCODE)."
        exit 1
    }
    if (-not (Test-Path $BinPath)) { Write-Error "Built binary not found at $BinPath."; exit 1 }
}

# The stdio child inherits these from our process env.
$env:UNIVERSE_STORE   = $StorePath
$env:UNIVERSE_GENESIS = $GenesisPath

# --- child process management ------------------------------------------------
$script:Children = @{}   # name -> System.Diagnostics.Process

# npx and ngrok are .cmd shims / WindowsApps aliases: CreateProcess cannot
# launch them directly, and ProcessStartInfo.ArgumentList is absent on Windows
# PowerShell 5.1. So every child runs through `cmd.exe /c <command line>`, which
# resolves shims from PATH and keeps a single supervisable process handle.
function Start-Child([string]$name, [string]$cmdline) {
    $p = Start-Process -FilePath $env:ComSpec `
                       -ArgumentList "/c $cmdline" `
                       -PassThru -NoNewWindow    # inherit console so logs/URLs show
    $script:Children[$name] = $p
    Write-Host "[up] $name pid=$($p.Id)" -ForegroundColor Green
    return $p
}

function Stop-All {
    Write-Host "`n[shutdown] stopping children ..." -ForegroundColor Cyan
    foreach ($kv in $script:Children.GetEnumerator()) {
        $p = $kv.Value
        if ($p -and -not $p.HasExited) {
            try { taskkill /PID $p.Id /T /F *> $null } catch {}
        }
    }
}

# The bridge command depends on the transport:
#  - streamableHttp (/mcp, what ChatGPT wants): our own dependency-free Node
#    bridge. supergateway's streamable-HTTP transport (stateful AND stateless)
#    throws "No connection established for request ID" and dies the moment a real
#    client drives it through ngrok; scripts/http-bridge.mjs never does that.
#  - sse (/sse + /message): supergateway, which works fine for this transport.
if ($Transport -eq 'sse') {
    $SgCmd = "npx -y supergateway --stdio `"$BinPath`" --port $Port --healthEndpoint /healthz" +
             " --outputTransport sse --ssePath /sse --messagePath /message"
} else {
    $env:BRIDGE_BIN  = $BinPath
    $env:BRIDGE_PORT = "$Port"
    $env:BRIDGE_PATH = '/mcp'
    $BridgeJs = Join-Path $RepoRoot 'scripts\http-bridge.mjs'
    $SgCmd = "node `"$BridgeJs`""
}

# ngrok: publish the bridge on the reserved domain. The whole host is forwarded,
# so the public URL that speaks MCP is https://$Domain$Path.
$NgrokCmd = "ngrok http $Port --domain=$Domain --log=stdout"

# Ctrl+C -> clean teardown. Non-fatal when there is no console handle (the
# script launched headless / with redirected stdio, e.g. as a background
# service): there is no Ctrl+C to trap in that case, and the supervisor below
# must still run.
try { [Console]::TreatControlCAsInput = $false } catch { }
$null = Register-EngineEvent -SourceIdentifier PowerShell.Exiting -Action { Stop-All }
try {
    Start-Child 'supergateway' $SgCmd    | Out-Null
    Start-Sleep -Seconds 2
    Start-Child 'ngrok'        $NgrokCmd | Out-Null

    Write-Host ""
    Write-Host "MCP is live -> https://$Domain$Path" -ForegroundColor Green
    Write-Host "Local inspector -> http://127.0.0.1:4040 (ngrok)  |  health -> http://127.0.0.1:$Port/healthz"
    Write-Host "Press Ctrl+C to stop.`n"

    # --- supervision loop: restart whichever child exits (bounded backoff) ---
    $backoff = @{ 'supergateway' = 0; 'ngrok' = 0 }
    while ($true) {
        Start-Sleep -Seconds 2
        foreach ($name in @('supergateway', 'ngrok')) {
            $p = $script:Children[$name]
            if ($p.HasExited) {
                $backoff[$name] = [Math]::Min($backoff[$name] + 2, 30)
                Write-Warning "[restart] $name exited (code $($p.ExitCode)); restarting in $($backoff[$name])s"
                Start-Sleep -Seconds $backoff[$name]
                if ($name -eq 'supergateway') { Start-Child $name $SgCmd    | Out-Null }
                else                          { Start-Child $name $NgrokCmd | Out-Null }
            } elseif ($backoff[$name] -gt 0) {
                $backoff[$name] = 0   # healthy again -> reset backoff
            }
        }
    }
}
finally {
    Stop-All
}
