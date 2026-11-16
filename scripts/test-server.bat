@echo off
rem ======================================================================
rem  Tests a maarse-server from this PC:
rem   1. can the server's backbone port be reached?
rem   2. starts two test hosts ("Test A" and "Test B") that only connect to
rem      the server (no radio, no listening port, as hosts behind NAT), and
rem      opens their web clients side by side.
rem  Write from one to the other: the messages go through the server.
rem  The test hosts keep their data in test-data\ next to this file, apart
rem  from your real host. Close their windows (or press a key here) to stop.
rem ======================================================================

rem The server to test (host:port of its backbone). Another address can be
rem given on the command line, e.g. the server's LAN address when the public
rem one cannot be reached from inside the same network (NAT hairpinning):
rem   test-server.bat 192.168.20.50:4242
set SERVER=nj01.dk.eu.maarse.dk:4242
if not "%~1"=="" set SERVER=%~1

rem Web ports for the two test hosts on this PC.
set PORT_A=8091
set PORT_B=8092
rem ======================================================================

cd /d "%~dp0"
set EXE=
if exist "%~dp0maarse-host.exe" set EXE=%~dp0maarse-host.exe
if not defined EXE if exist "%~dp0..\target\release\maarse-host.exe" set EXE=%~dp0..\target\release\maarse-host.exe
if not defined EXE (
  echo maarse-host.exe was not found next to this file or in target\release.
  echo Build it with: cargo build --release -p maarse-host --bin maarse-host
  pause
  exit /b 1
)

for /f "tokens=1,2 delims=:" %%a in ("%SERVER%") do (set SRV_HOST=%%a& set SRV_PORT=%%b)
echo.
echo == 1. Can %SRV_HOST% port %SRV_PORT% be reached?
powershell -NoProfile -Command "$r = Test-NetConnection -ComputerName '%SRV_HOST%' -Port %SRV_PORT% -WarningAction SilentlyContinue; if ($r.TcpTestSucceeded) { Write-Host ('   yes: ' + $r.RemoteAddress + ' answers on port %SRV_PORT%') -ForegroundColor Green; exit 0 } else { Write-Host ('   NO: ' + $r.RemoteAddress + ' does not answer on port %SRV_PORT% (port forwarding, firewall, or the server is not running)') -ForegroundColor Red; exit 1 }"
if errorlevel 1 (
  echo.
  echo If this PC is behind the same router/firewall as the server, its public
  echo address often cannot be reached from inside ^(NAT hairpinning^). Try the
  echo server's LAN address instead ^(on the server: hostname -I^), e.g.:
  echo    %~nx0 192.168.20.50:4242
  echo Otherwise check on the server: systemctl status maarse-server
  pause
  exit /b 1
)

echo.
echo == 2. Starting two test hosts that connect only to the server
if not exist "%~dp0test-data" mkdir "%~dp0test-data"
start "Test A (maarse-host, port %PORT_A%)" "%EXE%" --data "%~dp0test-data\a" --no-browser --listen off --http 127.0.0.1:%PORT_A% --peer %SERVER%
start "Test B (maarse-host, port %PORT_B%)" "%EXE%" --data "%~dp0test-data\b" --no-browser --listen off --http 127.0.0.1:%PORT_B% --peer %SERVER%
timeout /t 4 /nobreak >nul
start "" http://127.0.0.1:%PORT_A%/
start "" http://127.0.0.1:%PORT_B%/

echo.
echo Two browser tabs opened: Test A (port %PORT_A%) and Test B (port %PORT_B%).
echo  - In each: Settings - Name - give it a name (e.g. "Test A"), then press Announce.
echo  - After a few seconds they appear under Nodes in each other's client.
echo  - Write from Test A to Test B: the message goes through %SERVER%.
echo  - On the server's dashboard: Nodes ^& map, Live traffic and Links show both.
echo  - Mail test: close Test B's window, write to it from Test A, wait a minute,
echo    start this file again: Test B fetches the message from the post office.
echo.
echo Press a key to stop both test hosts.
pause >nul
taskkill /fi "WINDOWTITLE eq Test A (maarse-host*" /t /f >nul 2>nul
taskkill /fi "WINDOWTITLE eq Test B (maarse-host*" /t /f >nul 2>nul
echo Stopped.
