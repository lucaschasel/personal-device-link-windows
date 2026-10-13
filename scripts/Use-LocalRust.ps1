# Dot-source this script to use the optional project-local toolchain:
# . .\scripts\Use-LocalRust.ps1
$projectRoot = Split-Path -Parent $PSScriptRoot
$projectCargo = Join-Path $projectRoot '.tools\cargo'
$projectRustup = Join-Path $projectRoot '.tools\rustup'
$projectCargoBin = Join-Path $projectCargo 'bin'
if (-not (Test-Path -LiteralPath (Join-Path $projectCargoBin 'cargo.exe'))) {
    throw 'No project-local Rust installation found. Install Rust normally and use cargo from PATH.'
}
$env:CARGO_HOME = $projectCargo
$env:RUSTUP_HOME = $projectRustup
$env:PATH = "$projectCargoBin;$env:PATH"
