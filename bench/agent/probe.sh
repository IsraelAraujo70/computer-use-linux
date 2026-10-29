#!/usr/bin/env bash
# Open one bench page in a throwaway Chrome on the bench display, run MCP calls
# against a binary, then close Chrome. For live checks between benchmark runs.
# Usage: probe.sh <binary> <url-or-apps-path> <tool> <json> [<tool> <json>]...
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$1 page=$2; shift 2
[[ "$page" == *://* ]] || page="http://127.0.0.1:8765/apps/$page"
"$here/env.sh" start >/dev/null
source <("$here/env.sh" vars)
curl -sf http://127.0.0.1:8765/healthz >/dev/null || { mkdir -p /tmp/agent-cu-bench/state; nohup python3 "$here/server.py" --port 8765 --state-dir /tmp/agent-cu-bench/state >/dev/null 2>&1 & sleep 1; }
curl -sf -X POST http://127.0.0.1:8765/api/reset >/dev/null
profile=$(mktemp -d /tmp/agent-cu-bench/probe-XXXX)
chrome=$(ls -d "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux64/chrome | sort -V | tail -1)
"$chrome" --user-data-dir="$profile" --no-first-run --no-default-browser-check --force-renderer-accessibility "$page" >/dev/null 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; sleep 0.5; rm -rf "$profile"' EXIT
sleep 4
python3 "$here/mcpcall.py" "$bin" "$@"
