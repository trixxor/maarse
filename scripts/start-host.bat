@echo off
rem Builds (the first time) and starts maarse-host. Extra options are passed on, e.g.:
rem   scripts\start-host.bat --peer 192.168.1.20:4242
cd /d "%~dp0\.."
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is missing. Install it from https://rustup.rs and run the script again.
  pause
  exit /b 1
)
cargo run --release -p maarse-host -- %*
