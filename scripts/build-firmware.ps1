# Builds Maarse firmware (Windows).
#   .\scripts\build-firmware.ps1 wismesh-tag          -> dist\maarse-wismesh-tag.uf2
#   .\scripts\build-firmware.ps1 xiao-nrf52840
#   .\scripts\build-firmware.ps1 heltec-v4 [-Flash]   -> dist\maarse-heltec-v4.bin
#   .\scripts\build-firmware.ps1 xiao-esp32s3 [-Flash]
param(
    [Parameter(Mandatory = $true)][ValidateSet("wismesh-tag", "xiao-nrf52840", "heltec-v4", "xiao-esp32s3")][string]$Board,
    [switch]$Flash,
    [string]$Port
)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Host "Rust is missing: https://rustup.rs"; exit 1 }
New-Item -ItemType Directory -Force dist | Out-Null

# Signed package for over-the-air updates when MAARSE_SIGN_KEY is set
# (path to a key from `maarse-sign keygen`, or "dev"). Version from
# MAARSE_FW_VERSION (must also be set during the build).
function Sign-Image($img) {
    if (-not $env:MAARSE_SIGN_KEY) { return }
    if (-not $env:MAARSE_FW_VERSION) { Write-Host "Set MAARSE_FW_VERSION (e.g. 0.2.0) to sign"; exit 1 }
    $keyArg = @("--key", $env:MAARSE_SIGN_KEY)
    if ($env:MAARSE_SIGN_KEY -eq "dev") { $keyArg = @("--dev-key") }
    cargo run --release -q -p maarse-uf2 --bin maarse-sign -- sign --board $Board --version $env:MAARSE_FW_VERSION @keyArg $img "dist\maarse-$Board-ota"
}

if ($Board -in @("wismesh-tag", "xiao-nrf52840")) {
    rustup target add thumbv7em-none-eabihf
    # The Bluetooth part (nrf-sdc) uses bindgen, which needs libclang.
    if (-not $env:LIBCLANG_PATH) {
        $llvm = "C:\Program Files\LLVM\bin"
        if (-not (Test-Path "$llvm\libclang.dll")) {
            Write-Host "Installing LLVM (libclang) with winget."
            winget install --id LLVM.LLVM -e --silent --accept-package-agreements --accept-source-agreements
        }
        $env:LIBCLANG_PATH = $llvm
    }
    Push-Location firmware\nrf
    cargo build --release --features "board-$Board"
    if ($LASTEXITCODE -ne 0) { Pop-Location; exit 1 }
    Pop-Location
    cargo run --release -q -p maarse-uf2 -- firmware\nrf\target\thumbv7em-none-eabihf\release\maarse-fw-nrf "dist\maarse-$Board.uf2"
    Write-Host "Done: dist\maarse-$Board.uf2"
    Sign-Image "dist\maarse-$Board.uf2"
    if ($Flash) {
        if (-not $Port) { Write-Host "Give the port with -Port, e.g. -Port COM7"; exit 1 }
        python scripts\flash-nrf.py $Port "dist\maarse-$Board.uf2"
    } else {
        Write-Host "Flash with:  .\scripts\build-firmware.ps1 $Board -Flash -Port COMx"
        Write-Host "(or double-click reset and drag the file onto the USB drive)"
    }
    exit 0
}

# ESP32-S3
if (-not ((rustup toolchain list) -match '^esp')) {
    Write-Host "Installing Espressif's Rust toolchain (espup) - takes a few minutes the first time."
    if (-not (Get-Command espup -ErrorAction SilentlyContinue)) { cargo install espup --locked }
    espup install --targets esp32s3
}
$exp = Join-Path $HOME "export-esp.ps1"
if (Test-Path $exp) { . $exp }
if (-not (Get-Command espflash -ErrorAction SilentlyContinue)) { cargo install espflash --locked }
Push-Location firmware\esp
$features = "board-$Board"
cargo +esp build --release --features $features
if ($LASTEXITCODE -ne 0) { Pop-Location; exit 1 }
Pop-Location
$elf = "firmware\esp\target\xtensa-esp32s3-none-elf\release\maarse-fw-esp"
# Flash size: Heltec V4 16 MB, XIAO ESP32-S3 8 MB (the partition table needs more than 4 MB).
$flashSize = if ($Board -eq "heltec-v4") { "16mb" } else { "8mb" }
Remove-Item "dist\maarse-$Board.bin" -ErrorAction SilentlyContinue
espflash save-image --chip esp32s3 --merge --flash-size $flashSize --partition-table firmware\esp\partitions.csv $elf "dist\maarse-$Board.bin"
if ($LASTEXITCODE -ne 0) { Write-Host "Could not make the merged image"; exit 1 }
Write-Host "Done: dist\maarse-$Board.bin (merged image, flashed from address 0x0)"
# The app image alone (for over-the-air updates)
espflash save-image --chip esp32s3 $elf "dist\maarse-$Board-app.bin"
Sign-Image "dist\maarse-$Board-app.bin"
if ($Flash) {
    $portArg = @(); if ($Port) { $portArg = @("--port", $Port) }
    # otadata is erased, so the bootloader starts ota_0 (the one we flash)
    espflash flash @portArg --chip esp32s3 --partition-table firmware\esp\partitions.csv --erase-parts otadata $elf
} else {
    Write-Host "Flash with:  .\scripts\build-firmware.ps1 $Board -Flash"
}
