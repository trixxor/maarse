@echo off
rem Starts the Denmark simulator with "Ask Claude" (local Claude Code) and opens it.
cd /d "%~dp0.."
start "" http://127.0.0.1:8765/denmark/
node tools\sim-bridge\bridge.mjs
