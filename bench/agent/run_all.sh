#!/usr/bin/env bash
# One command for the pre-registered comparison (METRIC.md): both suites,
# baseline vs candidate interleaved, Chrome started like the bots' chrome-perfil
# plus --force-renderer-accessibility, then the summaries with the bar checks.
# Usage: run_all.sh <out-dir> <baseline-binary> <candidate-binary> [codex|claude] [reps]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$1 base=$2 cand=$3 agent=${4:-codex} reps=${5:-3}
# Optional env: MODEL, EFFORT (agent model/effort), MIN_RATIO (throughput bar, default 2.0).
extra=()
[[ -n "${MODEL:-}" ]] && extra+=(--model "$MODEL")
[[ -n "${EFFORT:-}" ]] && extra+=(--effort "$EFFORT")
"$here/run.py" --out "$out/easy" --suite easy --agent "$agent" --reps "$reps" --chrome-mode bots "${extra[@]}" \
  --variant "baseline=$base" --variant "final=$cand"
"$here/run.py" --out "$out/hard" --suite hard --agent "$agent" --reps "$reps" --chrome-mode bots --timeout 300 "${extra[@]}" \
  --variant "baseline=$base" --variant "final=$cand"
echo "## easy"; "$here/summarize.py" "$out/easy/runs.jsonl" --base baseline --cand final --min-ratio "${MIN_RATIO:-2.0}" --md "$out/easy/summary.md"
echo "## hard"; "$here/summarize.py" "$out/hard/runs.jsonl" --base baseline --cand final --min-ratio "${MIN_RATIO:-2.0}" --md "$out/hard/summary.md"
