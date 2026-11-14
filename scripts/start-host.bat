@echo off
rem Bygger (foerste gang) og starter maarse-host. Ekstra valg gives videre, fx:
rem   scripts\start-host.bat --peer 192.168.1.20:4242
cd /d "%~dp0\.."
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust mangler. Installer det fra https://rustup.rs og koer scriptet igen.
  pause
  exit /b 1
)
cargo run --release -p maarse-host -- %*
