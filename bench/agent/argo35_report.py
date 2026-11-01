#!/usr/bin/env python3
"""Per-category and per-task tables for argo35 runs, and the argo35 part of the bar (METRIC.md).

Usage: argo35_report.py <runs.jsonl>... [--cand NAME] [--md out.md]
"""
import argparse
import statistics
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import summarize  # noqa: E402
import tasks_argo35  # noqa: E402


def table(rows: list[list[str]]) -> str:
    head, *body = rows
    return "\n".join(["| " + " | ".join(head) + " |", "|" + "---|" * len(head)] + ["| " + " | ".join(r) + " |" for r in body])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--cand", help="variant that must pass every episode (the bar)")
    ap.add_argument("--md")
    args = ap.parse_args()
    runs = [r for r in summarize.load(args.runs) if r["task"].startswith("a35-")]
    variants = sorted({r["variant"] for r in runs})
    holdout = {t.id for t in tasks_argo35.tasks("holdout")}
    by = defaultdict(list)
    for r in runs:
        by[(r["variant"], r["task"])].append(r)
    out = []
    rows = [["variant", "episodes", "success", "train", "holdout", "agent min", "succ/min", "median s", "calls"]]
    for v in variants:
        rs = [r for r in runs if r["variant"] == v]
        tr = [r for r in rs if r["task"] not in holdout]
        ho = [r for r in rs if r["task"] in holdout]
        o = summarize.overall(rs)
        rows.append([v, str(len(rs)), f"{o['successes']}/{len(rs)}", f"{sum(r['ok'] for r in tr)}/{len(tr)}",
                     f"{sum(r['ok'] for r in ho)}/{len(ho)}", f"{o['agent_minutes']:.1f}", f"{o['throughput']:.2f}",
                     f"{statistics.median(r['wall_s'] for r in rs):.0f}", f"{o['mean_mcp_calls']:.1f}"])
    out += ["## argo35 overall", table(rows), ""]
    cats = defaultdict(set)
    for r in runs:
        cats[tasks_argo35.category(r["task"])].add(r["task"])
    rows = [["category", "tasks"] + variants]
    for c in tasks_argo35.CATEGORIES:
        if c not in cats:
            continue
        cells = []
        for v in variants:
            rs = [r for t in cats[c] for r in by[(v, t)]]
            cells.append(f"{sum(r['ok'] for r in rs)}/{len(rs)}" if rs else "-")
        rows.append([c, str(len(cats[c]))] + cells)
    out += ["## per category (successful episodes)", table(rows), ""]
    rows = [["task", "set"] + [f"{v} ok / median s / calls" for v in variants]]
    for t in sorted({r["task"] for r in runs}, key=lambda t: (tasks_argo35.category(t), t)):
        cells = []
        for v in variants:
            rs = by[(v, t)]
            cells.append(f"{sum(r['ok'] for r in rs)}/{len(rs)} / {statistics.median(r['wall_s'] for r in rs):.0f} / "
                         f"{statistics.fmean(r['mcp_calls'] for r in rs):.0f}" if rs else "-")
        rows.append([t, "holdout" if t in holdout else "train"] + cells)
    out += ["## per task", table(rows), ""]
    fails = [r for r in runs if not r["ok"]]
    if fails:
        out += ["## failures"] + [f"- {r['run_id']}: {r['detail'][:160]}" + (" (timeout)" if r.get("timed_out") else "")
                                   for r in sorted(fails, key=lambda r: r["run_id"])] + [""]
    if args.cand:
        cand = [r for r in runs if r["variant"] == args.cand]
        tasks = {r["task"] for r in cand}
        reps = defaultdict(int)
        for r in cand:
            reps[r["task"]] += 1
        met = len(tasks) == 35 and all(reps[t] >= 3 for t in tasks) and all(r["ok"] for r in cand)
        out.append(f"argo35 bar (35 tasks x 3 reps, every episode passes) for {args.cand}: "
                   f"{sum(r['ok'] for r in cand)}/{len(cand)} episodes, {len(tasks)} tasks -> met: {met}")
    text = "\n".join(out)
    print(text)
    if args.md:
        Path(args.md).write_text(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
