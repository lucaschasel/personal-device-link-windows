$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
if (Test-Path -LiteralPath (Join-Path $projectRoot '.tools/cargo/bin/cargo.exe')) {
    . (Join-Path $PSScriptRoot 'Use-LocalRust.ps1')
}
Push-Location (Join-Path $projectRoot 'desktop')
try {
    npm.cmd ci --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    npm.cmd run desktop:build -- --no-bundle -- --locked
    if ($LASTEXITCODE -ne 0) { throw 'Desktop build failed' }
} finally { Pop-Location }
Push-Location $projectRoot
try {
    cargo build --release --bin bluetooth_bridge --locked
    if ($LASTEXITCODE -ne 0) { throw 'Bluetooth bridge build failed' }
} finally { Pop-Location }
& (Join-Path $PSScriptRoot 'Package-Desktop.ps1')
