@echo off
rem Serves docs/ and opens the Denmark simulator (browsers do not load .wasm from file://).
cd /d "%~dp0.."
start "" http://127.0.0.1:8000/denmark/
python -m http.server -d docs 8000
