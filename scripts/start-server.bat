@echo off
rem ======================================================================
rem  Starts maarse-server: the dedicated host for a server or VPS
rem  (backbone, relay for hosts behind NAT, post office copies and the
rem  telemetry dashboard with admin login). See docs\GUIDE.md,
rem  "Dedicated server (maarse-server)".
rem
rem  Fill in the settings below and run the file. Extra options on the
rem  command line are passed on, e.g.:  start-server.bat --min-peers 8
rem ======================================================================

rem ---- Required ---------------------------------------------------------
rem The address and port others reach this server on (your public IP or a
rem DNS name). Open this TCP port in the router/firewall.
set PUBLIC_ADDR=nj01.dk.eu.maarse.dk:4242

rem ---- Network ----------------------------------------------------------
rem Port the backbone listens on (must match the port in PUBLIC_ADDR).
set LISTEN=0.0.0.0:4242
rem Other servers or hosts to connect to, separated by spaces (may be empty).
set PEERS=
rem A DNS name with several servers behind it (may be empty).
set SEED_DNS=seed.maarse.dk
rem Keep at least this many backbone links.
set MIN_PEERS=6

rem ---- Regions (may be empty = forward everything) ----------------------
rem Regions this server forwards channel messages for, separated by spaces,
rem e.g. dk nordjylland. STRICT=yes: without regions only messages
rem without a region.
set REGIONS=
set STRICT=no

rem ---- Dashboard --------------------------------------------------------
rem Address of the admin dashboard. 0.0.0.0:8080 = reachable from outside
rem (use HTTPS through a reverse proxy over the internet);
rem 127.0.0.1:8080 = only this machine / an SSH tunnel.
set HTTP=0.0.0.0:8080
rem Set a new admin password at start (empty = keep the current one; the
rem first password is printed once and written to admin-password.txt).
set ADMIN_PASSWORD=

rem ---- Optional: a LoRa modem on this machine (empty = none, as on a VPS)
set SERIAL=

rem ---- Data folder (identity, telemetry, mail, admin password) ----------
set DATA=maarse-server-data
rem ======================================================================

cd /d "%~dp0"
set EXE=
if exist "%~dp0maarse-server.exe" set EXE=%~dp0maarse-server.exe
if not defined EXE if exist "%~dp0..\target\release\maarse-server.exe" set EXE=%~dp0..\target\release\maarse-server.exe
if not defined EXE (
  where cargo >nul 2>nul
  if errorlevel 1 (
    echo maarse-server.exe was not found next to this file, and Rust is missing to build it.
    echo Install Rust from https://rustup.rs, or put maarse-server.exe next to this file.
    pause
    exit /b 1
  )
  echo Building maarse-server (first time only^) ...
  pushd "%~dp0.."
  cargo build --release -p maarse-host --bin maarse-server || (popd & pause & exit /b 1)
  popd
  set EXE=%~dp0..\target\release\maarse-server.exe
)

if "%PUBLIC_ADDR%"=="server.example.org:4242" (
  echo Fill in PUBLIC_ADDR at the top of %~nx0 first: the address others reach this server on.
  pause
  exit /b 1
)

if defined ADMIN_PASSWORD (
  "%EXE%" --set-admin-password "%ADMIN_PASSWORD%" --data "%DATA%" || (pause & exit /b 1)
)

set ARGS=--data "%DATA%" --public-addr %PUBLIC_ADDR% --listen %LISTEN% --http %HTTP% --min-peers %MIN_PEERS%
for %%p in (%PEERS%) do call set ARGS=%%ARGS%% --peer %%p
for %%s in (%SEED_DNS%) do call set ARGS=%%ARGS%% --seed-dns %%s
for %%r in (%REGIONS%) do call set ARGS=%%ARGS%% --region %%r
if /i "%STRICT%"=="yes" set ARGS=%ARGS% --strict-region
if defined SERIAL set ARGS=%ARGS% --serial %SERIAL%

echo Starting: "%EXE%" %ARGS% %*
"%EXE%" %ARGS% %*
pause
