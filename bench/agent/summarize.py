#!/usr/bin/env python3
"""Summarize runs.jsonl files into the pre-registered metric (see METRIC.md).

Usage: summarize.py <runs.jsonl>... [--base NAME --cand NAME] [--md out.md]
Variants are grouped by their `variant` field across all files given.
"""
import argparse
import json
import statistics
from collections import defaultdict
from pathlib import Path


def load(paths):
    runs = []
    for path in paths:
        for line in Path(path).read_text().splitlines():
            if line.strip():
                runs.append(json.loads(line))
    return runs


def mean(xs):
    return statistics.fmean(xs) if xs else 0.0


def overall(runs):
    ok = sum(r["ok"] for r in runs)
    minutes = sum(r["wall_s"] for r in runs) / 60
    return {
        "runs": len(runs),
        "successes": ok,
        "success_rate": ok / len(runs) if runs else 0.0,
        "agent_minutes": minutes,
        "throughput": ok / minutes if minutes else 0.0,  # successful tasks per agent-minute (primary)
        "mean_wall_s": mean([r["wall_s"] for r in runs]),
        "mean_mcp_calls": mean([r["mcp_calls"] for r in runs]),
        "mean_screenshots": mean([r["screenshots"] for r in runs]),
        "mean_mcp_kb": mean([r.get("mcp_bytes_total", 0) / 1000 for r in runs]),
        "mean_input_tokens": mean([r["tokens"]["input"] for r in runs]),
        "mean_mcp_ms": mean([r["mcp_ms_total"] for r in runs]),
        "mean_ms_per_call": (sum(r["mcp_ms_total"] for r in runs) / max(1, sum(r["mcp_calls"] for r in runs))),
        "timeouts": sum(r.get("timed_out", False) for r in runs),
    }


def per_task(runs):
    by = defaultdict(list)
    for r in runs:
        by[r["task"]].append(r)
    return {task: {"n": len(rs), "ok": sum(r["ok"] for r in rs),
                   "median_wall_s": statistics.median([r["wall_s"] for r in rs]),
                   "mean_calls": mean([r["mcp_calls"] for r in rs]),
                   "mean_shots": mean([r["screenshots"] for r in rs]),
                   "mean_tokens": mean([r["tokens"]["input"] for r in rs])}
            for task, rs in sorted(by.items())}


def passing(task_row):
    """A task 'passes' for a variant when a strict majority of its runs succeed."""
    return task_row["ok"] * 2 > task_row["n"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--base")
    ap.add_argument("--cand")
    ap.add_argument("--md")
    ap.add_argument("--min-ratio", type=float, default=2.0, help="throughput ratio the bar requires")
    args = ap.parse_args()
    runs = load(args.files)
    by_variant = defaultdict(list)
    for r in runs:
        by_variant[r["variant"]].append(r)
    lines = []
    lines.append("| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | MCP KB | input tokens | MCP ms/call |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|")
    summary = {}
    for name, rs in by_variant.items():
        o = overall(rs)
        summary[name] = o
        lines.append(f"| {name} | {o['runs']} | {o['successes']} | {o['success_rate']:.0%} | {o['agent_minutes']:.1f} | "
                     f"**{o['throughput']:.3f}** | {o['mean_wall_s']:.0f} | {o['mean_mcp_calls']:.1f} | "
                     f"{o['mean_screenshots']:.1f} | {o['mean_mcp_kb']:.1f} | {o['mean_input_tokens']/1000:.0f}k | {o['mean_ms_per_call']:.0f} |")
    lines.append("")
    names = list(by_variant)
    tasks_by_variant = {name: per_task(rs) for name, rs in by_variant.items()}
    all_tasks = sorted({t for v in tasks_by_variant.values() for t in v})
    header = "| task | " + " | ".join(f"{n} ok | {n} median s | {n} calls | {n} shots" for n in names) + " |"
    lines.append(header)
    lines.append("|---|" + "---|" * (4 * len(names)))
    for task in all_tasks:
        cells = []
        for n in names:
            row = tasks_by_variant[n].get(task)
            cells += ([f"{row['ok']}/{row['n']}", f"{row['median_wall_s']:.0f}", f"{row['mean_calls']:.1f}",
                       f"{row['mean_shots']:.1f}"] if row else ["-"] * 4)
        lines.append(f"| {task} | " + " | ".join(cells) + " |")
    if args.base and args.cand and args.base in summary and args.cand in summary:
        b, c = summary[args.base], summary[args.cand]
        ratio = c["throughput"] / b["throughput"] if b["throughput"] else float("inf")
        regressed = [t for t in all_tasks
                     if t in tasks_by_variant[args.base] and passing(tasks_by_variant[args.base][t])
                     and not (t in tasks_by_variant[args.cand] and passing(tasks_by_variant[args.cand][t]))]
        lines.append("")
        lines.append(f"**{args.cand} vs {args.base}: tasks/min ratio = {ratio:.2f}x**; "
                     f"success rate {b['success_rate']:.0%} -> {c['success_rate']:.0%}; "
                     f"tasks regressed pass->fail: {regressed or 'none'}; "
                     f"bar met (>= {args.min_ratio}x): {ratio >= args.min_ratio and c['success_rate'] >= b['success_rate'] and not regressed}")
    text = "\n".join(lines)
    print(text)
    if args.md:
        Path(args.md).write_text(text + "\n")


if __name__ == "__main__":
    main()
