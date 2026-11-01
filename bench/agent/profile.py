#!/usr/bin/env python3
"""Split each episode's agent wall time into boot, model turns and MCP time.

Usage: profile.py <run-dir>... [--md out.md]
A run dir holds runs/<run_id>/{result.json,mcp.jsonl}. Needs result.json
`started_at`/`ended_at` (run_one.py records them); older runs fall back to
the result file's mtime minus the check and teardown (~1 s), marked "~".

Timeline of one episode, from mcp.jsonl timestamps (each entry is written
when the server answered; its start is t - ms):
  boot   = agent launch -> tools/list answered (CLI start, MCP spawn, catalog)
  model  = gaps between one answered call and the next call's request, plus
           tools/list -> first call and last call -> agent exit (final reply)
  mcp    = time the server spent answering tools/call
Model turns = tools/call count + 1 (the final reply).
"""
import argparse
import json
import statistics
from collections import defaultdict
from pathlib import Path


def episode(run: Path):
    res = json.loads((run / "result.json").read_text())
    log = run / "mcp.jsonl"
    entries = [json.loads(l) for l in log.read_text().splitlines()] if log.exists() else []
    calls = [e for e in entries if e.get("method") == "tools/call"]
    listed = next((e for e in entries if e.get("method") == "tools/list"), None)
    approx = "started_at" not in res
    end = res.get("ended_at") or ((run / "result.json").stat().st_mtime - 1.0)
    start = res.get("started_at") or (end - res["wall_s"])
    boot = (listed["t"] - start) if listed else None
    gaps = []
    prev = listed["t"] if listed else start
    for c in calls:
        begin = c["t"] - c["ms"] / 1000
        gaps.append(max(0.0, begin - prev))
        prev = c["t"]
    final = max(0.0, end - prev)
    mcp_s = sum(c["ms"] for c in calls) / 1000
    return {
        "run": run.name, "task": res["task"], "variant": res["variant"], "ok": res["ok"],
        "wall": res["wall_s"], "boot": boot, "first": gaps[0] if gaps else None,
        "model_gaps": gaps, "final": final, "model_total": sum(gaps) + final, "mcp": mcp_s,
        "calls": len(calls), "turns": len(calls) + 1, "kb": sum(c["bytes"] for c in calls) / 1000,
        "tools": [c["tool"] for c in calls], "slow": sorted(((c["ms"], c["tool"]) for c in calls), reverse=True)[:2],
        "approx": approx, "tokens_in": res["tokens"]["input"],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--md")
    args = ap.parse_args()
    eps = []
    for d in args.dirs:
        for run in sorted(Path(d).glob("runs/*")):
            if (run / "result.json").exists():
                eps.append(episode(run))
    lines = ["| run | ok | wall s | boot s | model s (turns) | s/turn | final s | MCP s | KB | slowest calls | tools |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for e in eps:
        per_turn = e["model_total"] / e["turns"]
        slow = ", ".join(f"{t} {ms/1000:.1f}" for ms, t in e["slow"])
        lines.append(f"| {e['run']} | {'ok' if e['ok'] else 'FAIL'} | {e['wall']:.0f}{'~' if e['approx'] else ''} | "
                     f"{e['boot'] or 0:.1f} | {e['model_total']:.1f} ({e['turns']}) | {per_turn:.1f} | {e['final']:.1f} | "
                     f"{e['mcp']:.1f} | {e['kb']:.1f} | {slow} | {' '.join(e['tools'])} |")
    by = defaultdict(list)
    for e in eps:
        by[e["variant"]].append(e)
    lines.append("")
    lines.append("| variant | eps | mean wall | boot | model | MCP | turns | s per model turn | KB | share boot/model/MCP |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|")
    for v, es in by.items():
        m = lambda k: statistics.fmean(e[k] or 0 for e in es)
        wall, boot, model, mcp = m("wall"), m("boot"), m("model_total"), m("mcp")
        turns = m("turns")
        lines.append(f"| {v} | {len(es)} | {wall:.1f} | {boot:.1f} | {model:.1f} | {mcp:.1f} | {turns:.1f} | "
                     f"{model/turns:.1f} | {m('kb'):.1f} | {boot/wall:.0%}/{model/wall:.0%}/{mcp/wall:.0%} |")
    text = "\n".join(lines)
    print(text)
    if args.md:
        Path(args.md).write_text(text + "\n")


if __name__ == "__main__":
    main()
