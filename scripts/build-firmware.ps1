# Bygger Maarse-firmware (Windows).
#   .\scripts\build-firmware.ps1 wismesh-tag          -> dist\maarse-wismesh-tag.uf2
#   .\scripts\build-firmware.ps1 xiao-nrf52840
#   .\scripts\build-firmware.ps1 heltec-v4 [-Flash]   -> dist\maarse-heltec-v4.bin
#   .\scripts\build-firmware.ps1 xiao-esp32s3 [-Flash]
param(
    [Parameter(Mandatory = $true)][ValidateSet("wismesh-tag", "xiao-nrf52840", "heltec-v4", "xiao-esp32s3")][string]$Board,
    [switch]$Flash
)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Host "Rust mangler: https://rustup.rs"; exit 1 }
New-Item -ItemType Directory -Force dist | Out-Null

if ($Board -in @("wismesh-tag", "xiao-nrf52840")) {
    rustup target add thumbv7em-none-eabihf
    Push-Location firmware\nrf
    cargo build --release --features "board-$Board"
    if ($LASTEXITCODE -ne 0) { Pop-Location; exit 1 }
    Pop-Location
    cargo run --release -q -p maarse-uf2 -- firmware\nrf\target\thumbv7em-none-eabihf\release\maarse-fw-nrf "dist\maarse-$Board.uf2"
    Write-Host "Faerdig: dist\maarse-$Board.uf2"
    Write-Host "Dobbeltklik paa reset, og traek filen over paa USB-drevet, der dukker op."
    exit 0
}

# ESP32-S3
if (-not ((rustup toolchain list) -match '^esp')) {
    Write-Host "Installerer Espressifs Rust-vaerktoejskaede (espup) - tager et par minutter foerste gang."
    if (-not (Get-Command espup -ErrorAction SilentlyContinue)) { cargo install espup --locked }
    espup install --targets esp32s3
}
$exp = Join-Path $HOME "export-esp.ps1"
if (Test-Path $exp) { . $exp }
if (-not (Get-Command espflash -ErrorAction SilentlyContinue)) { cargo install espflash --locked }
Push-Location firmware\esp
cargo +esp build --release --features "board-$Board"
if ($LASTEXITCODE -ne 0) { Pop-Location; exit 1 }
Pop-Location
$elf = "firmware\esp\target\xtensa-esp32s3-none-elf\release\maarse-fw-esp"
espflash save-image --chip esp32s3 --merge --partition-table firmware\esp\partitions.csv $elf "dist\maarse-$Board.bin"
Write-Host "Faerdig: dist\maarse-$Board.bin (samlet billede, flashes fra adresse 0x0)"
if ($Flash) {
    espflash flash --chip esp32s3 --partition-table firmware\esp\partitions.csv $elf
} else {
    Write-Host "Flash med:  .\scripts\build-firmware.ps1 $Board -Flash"
}
