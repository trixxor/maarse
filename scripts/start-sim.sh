#!/bin/sh
# Starts the Denmark simulator with "Ask Claude" (local Claude Code).
cd "$(dirname "$0")/.." && exec node tools/sim-bridge/bridge.mjs
