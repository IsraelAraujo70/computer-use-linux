#!/usr/bin/env python3
"""The no-regression half of the argo35 bar (METRIC.md, "argo35"): per suite, every task that passes
(strict majority) for d494628 in its pre-registered Sol 5x final run must also pass for the final binary.

Usage: sol_regression.py <final-out-dir>   (expects sol-easy/, sol-hard/, sol-long/ with runs.jsonl)
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import summarize  # noqa: E402

out = Path(sys.argv[1])
regressions, lines = [], []
for suite in ("easy", "hard", "long"):
    # The Sol 5x run's "final" variant is the 4334fc2 build, the same server source as d494628.
    base = [r for r in summarize.load([HERE / "results" / "sol-5x-final" / f"{suite}-runs.jsonl"]) if r["variant"] == "final"]
    cand = summarize.load([out / f"sol-{suite}" / "runs.jsonl"])
    pb, pc = summarize.per_task(base), summarize.per_task(cand)
    ob, oc = summarize.overall(base), summarize.overall(cand)
    lines.append(f"| {suite} | {ob['successes']}/{ob['runs']} | {oc['successes']}/{oc['runs']} | "
                 f"{ob['throughput']:.2f} | {oc['throughput']:.2f} | {oc['throughput'] / ob['throughput']:.2f}x |")
    for task, row in pb.items():
        if summarize.passing(row) and not (task in pc and summarize.passing(pc[task])):
            regressions.append(f"{suite}/{task}: d494628 {row['ok']}/{row['n']}, final "
                               f"{pc.get(task, {}).get('ok', 0)}/{pc.get(task, {}).get('n', 0)}")
print("| suite | d494628 success | final success | d494628 succ/min | final succ/min | ratio |")
print("|---|---|---|---|---|---|")
print("\n".join(lines))
print("pass->fail regressions:", regressions or "none")
print("no-regression bar met:", not regressions)
sys.exit(1 if regressions else 0)
