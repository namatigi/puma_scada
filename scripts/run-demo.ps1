<#
.SYNOPSIS
  Builds the release binaries and runs the OXOAFRICA SCADA demo: simulator, drivers,
  historian, alarms, HMI and studio in one process.

.EXAMPLE
  .\scripts\run-demo.ps1
  .\scripts\run-demo.ps1 -Listen 0.0.0.0:8080      # reachable from tablets on the LAN
  .\scripts\run-demo.ps1 -NoBuild                  # reuse the last release build
  .\scripts\run-demo.ps1 -Fresh                    # start with empty history and alarm journal
#>
[CmdletBinding()]
param(
    [string]$Listen = "127.0.0.1:8080",
    [switch]$NoBuild,
    [switch]$Fresh
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
. (Join-Path $PSScriptRoot "env.ps1")

$project = Join-Path $root "demo\oxoafrica"
$data = Join-Path $root "demo\data"
$web = Join-Path $root "web"
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$rt = Join-Path $targetDir "release\moviclone-rt.exe"
$movi = Join-Path $targetDir "release\movi.exe"

if (-not $NoBuild) {
    Write-Host "Building release binaries (the first build takes a few minutes)..." -ForegroundColor Cyan
    & cargo build --release -p moviclone-rt -p movi
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
if (-not (Test-Path $rt)) { throw "$rt not found; run without -NoBuild" }

Write-Host "Checking demo\oxoafrica..." -ForegroundColor Cyan
& $movi build $project
if ($LASTEXITCODE -ne 0) { throw "the demo project does not pass 'movi build'" }

if ($Fresh -and (Test-Path $data)) {
    Write-Host "Removing previous demo history and alarm journal ($data)" -ForegroundColor Yellow
    Remove-Item -Recurse -Force $data
}

$hostPart, $port = $Listen.Split(":")
$shown = if ($hostPart -eq "0.0.0.0") { "127.0.0.1" } else { $hostPart }
Write-Host ""
Write-Host "  HMI     http://${shown}:$port/hmi/" -ForegroundColor Green
Write-Host "  Studio  http://${shown}:$port/studio/" -ForegroundColor Green
Write-Host "  API     http://${shown}:$port/api/diagnostics" -ForegroundColor DarkGray
Write-Host "  Data    $data" -ForegroundColor DarkGray
Write-Host "  Ctrl+C stops the runtime." -ForegroundColor DarkGray
Write-Host ""

& $rt $project --simulate --studio --listen $Listen --web $web --data $data
exit $LASTEXITCODE
