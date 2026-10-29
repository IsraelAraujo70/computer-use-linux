#!/usr/bin/env python3
"""Benchmark driver: interleaves variants x tasks x reps, one episode at a time.

Example:
  bench/agent/run.py --out bench/agent/results/baseline \
      --variant baseline=$HOME/.local/share/computer-use-linux-fork/bin/computer-use-linux --reps 3

Each episode runs through the shared heavy.sh wrapper (one heavy job at a time
on this host). Results append to <out>/runs.jsonl; rerunning resumes.
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import tasks  # noqa: E402

HEAVY = Path("/home/israel/.cache/t3-testdata/heavy.sh")


def ensure_server(root: Path, port: int) -> None:
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=2).read()
        return
    except OSError:
        pass
    (root / "state").mkdir(parents=True, exist_ok=True)
    log = open(root / "server.log", "a")
    proc = subprocess.Popen([sys.executable, str(HERE / "server.py"), "--port", str(port),
                             "--state-dir", str(root / "state")], stdout=log, stderr=log,
                            start_new_session=True)
    (root / "server.pid").write_text(str(proc.pid))
    for _ in range(50):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1).read()
            return
        except OSError:
            time.sleep(0.1)
    raise SystemExit("task server did not start")


def ensure_codex_home(root: Path) -> Path:
    home = root / "codex-home"
    home.mkdir(parents=True, exist_ok=True)
    auth = home / "auth.json"
    if not auth.exists():
        auth.symlink_to(Path.home() / ".codex" / "auth.json")
    # Disable the servers the machine-wide managed config adds (one of them drives :99).
    (home / "config.toml").write_text(
        '[mcp_servers.node_repl]\ncommand = "/bin/false"\nenabled = false\n\n'
        '[mcp_servers.computer-use-linux]\ncommand = "/bin/false"\nenabled = false\n'
    )
    return home


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--variant", action="append", required=True, help="name=/path/to/binary")
    ap.add_argument("--tasks", default="all")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--agent", choices=["codex", "claude"], default="codex")
    ap.add_argument("--model", default=None)
    ap.add_argument("--effort", default=None)
    ap.add_argument("--timeout", type=int, default=360)
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--root", default=os.environ.get("BENCH_ROOT", "/tmp/agent-cu-bench"))
    ap.add_argument("--no-heavy", action="store_true", help="run episodes without heavy.sh")
    ap.add_argument("--no-chrome-a11y", action="store_true")
    args = ap.parse_args()

    model = args.model or ("gpt-6.1-terra" if args.agent == "codex" else "claude-sonnet-5-5")
    effort = args.effort or ("low" if args.agent == "codex" else "medium")
    variants = [tuple(v.split("=", 1)) for v in args.variant]
    for name, binary in variants:
        if not os.access(binary, os.X_OK):
            raise SystemExit(f"variant {name}: {binary} is not executable")
    task_ids = [t.id for t in tasks.TASKS] if args.tasks == "all" else args.tasks.split(",")
    out = Path(args.out).resolve()
    (out / "runs").mkdir(parents=True, exist_ok=True)
    root = Path(args.root)
    subprocess.run([str(HERE / "env.sh"), "start"], check=True, env={**os.environ, "BENCH_ROOT": str(root)})
    ensure_server(root, args.port)
    codex_home = ensure_codex_home(root)
    workdir = root / "work"
    workdir.mkdir(parents=True, exist_ok=True)
    done = set()
    runs_path = out / "runs.jsonl"
    if runs_path.exists():
        done = {json.loads(l)["run_id"] for l in runs_path.read_text().splitlines() if l.strip()}
    (out / "meta.json").write_text(json.dumps({"variants": dict(variants), "agent": args.agent, "model": model,
                                               "effort": effort, "tasks": task_ids, "reps": args.reps,
                                               "timeout": args.timeout}, indent=1))
    order = []
    for rep in range(args.reps):
        for i, task_id in enumerate(task_ids):
            shift = (rep + i) % len(variants)  # rotate which variant goes first
            for name, binary in variants[shift:] + variants[:shift]:
                order.append((rep, task_id, name, binary))
    for n, (rep, task_id, name, binary) in enumerate(order, 1):
        run_id = f"{name}-{task_id}-r{rep}"
        if run_id in done:
            continue
        run_dir = out / "runs" / run_id
        run_dir.mkdir(parents=True, exist_ok=True)
        for stale in ("mcp.jsonl", "result.json"):
            (run_dir / stale).unlink(missing_ok=True)
        spec = {"run_id": run_id, "variant": name, "binary": binary, "task": task_id, "rep": rep,
                "agent": args.agent, "model": model, "effort": effort, "timeout": args.timeout,
                "port": args.port, "root": str(root), "codex_home": str(codex_home), "workdir": str(workdir),
                "chrome_a11y": not args.no_chrome_a11y}
        (run_dir / "spec.json").write_text(json.dumps(spec, indent=1))
        cmd = [sys.executable, str(HERE / "run_one.py"), str(run_dir / "spec.json")]
        if not args.no_heavy:
            cmd = [str(HEAVY)] + cmd
        subprocess.run(cmd, cwd=HERE)
        result_path = run_dir / "result.json"
        if not result_path.exists():
            print(f"[{n}/{len(order)}] {run_id}: episode crashed (see {run_dir})", flush=True)
            continue
        result = json.loads(result_path.read_text())
        with open(runs_path, "a") as f:
            f.write(json.dumps({k: v for k, v in result.items() if k != "state"}, ensure_ascii=False) + "\n")
        print(f"[{n}/{len(order)}] {run_id}: {'PASS' if result['ok'] else 'FAIL'} "
              f"{result['wall_s']:.0f}s calls={result['mcp_calls']} shots={result['screenshots']} "
              f"tok={result['tokens']['input']} {'' if result['ok'] else result['detail'][:120]}", flush=True)


if __name__ == "__main__":
    main()
