# Build ebbtop.exe natively on Windows with the self-contained GNU toolchain
# (no Visual Studio; about 1 GB). Run from the repo root.
#
#   winget install Rustlang.Rustup      # once, if cargo is missing
#   powershell -ExecutionPolicy Bypass -File deploy\windows\build.ps1 [-Install] [-Autostart]
param([switch]$Install, [switch]$Autostart)
$ErrorActionPreference = 'Stop'
$tc = 'stable-x86_64-pc-windows-gnu'

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw "rustup not found. Run: winget install Rustlang.Rustup  (then open a new shell)"
}
rustup toolchain install $tc --profile minimal
if ($LASTEXITCODE -ne 0) { throw "toolchain install failed" }

cargo "+$tc" build --release --locked -p ebbtop-gui -p ebbtop-core
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$exe = Join-Path (Get-Location) 'target\release\ebbtop.exe'
$size = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "built $exe ($size MB)"
Write-Host "probe: target\release\ebbtop-probe.exe --seconds 60 --token-file <file>"

if ($Install) {
    & (Join-Path $PSScriptRoot 'install.ps1') -Exe $exe -Autostart:$Autostart
}
