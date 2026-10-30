#!/usr/bin/env bash
# One command for the pre-registered comparison (METRIC.md): both suites,
# baseline vs candidate interleaved, Chrome started like the bots' chrome-perfil
# plus --force-renderer-accessibility, then the summaries with the bar checks.
# Usage: run_all.sh <out-dir> <baseline-binary> <candidate-binary> [codex|claude] [reps]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$1 base=$2 cand=$3 agent=${4:-codex} reps=${5:-3}
"$here/run.py" --out "$out/easy" --suite easy --agent "$agent" --reps "$reps" --chrome-mode bots \
  --variant "baseline=$base" --variant "final=$cand"
"$here/run.py" --out "$out/hard" --suite hard --agent "$agent" --reps "$reps" --chrome-mode bots --timeout 300 \
  --variant "baseline=$base" --variant "final=$cand"
echo "## easy"; "$here/summarize.py" "$out/easy/runs.jsonl" --base baseline --cand final --md "$out/easy/summary.md"
echo "## hard"; "$here/summarize.py" "$out/hard/runs.jsonl" --base baseline --cand final --md "$out/hard/summary.md"
