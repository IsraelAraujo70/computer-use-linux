#!/usr/bin/env python3
"""Run one task episode, then resume the same agent session and interview it.

Usage: interview.py <out-dir> <binary> <task> [--agent codex|claude] [--model M] [--effort E]
Writes <out-dir>/<task>/{agent.jsonl,mcp.jsonl,result.json,interview.txt}.
The interview turn has no tools; it only asks the agent about the episode it just did.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run_one  # noqa: E402
import tasks  # noqa: E402

QUESTIONS = """Interview about the task you just did. Make no more tool calls; answer from what happened in this session, in short concrete bullets.
1. What slowed you down, or where did you have to guess (coordinates, which element, whether an action worked)?
2. Which of your tool calls were wasted or repeated, and why did you make them?
3. For this task, compare (a) reading the accessibility tree/outline, (b) acting on an element by index or by role/name without screen coordinates, (c) taking screenshots. Which did you rely on, which would you drop, and what was missing from each?
4. What one new tool, or change to a tool's output, would have helped you most? Describe its inputs and what it should return.
5. Was anything in tool descriptions, results, errors or diagnostics confusing or misleading?
6. Anything about speed (slow tool responses, too much output to read)?"""


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("binary")
    ap.add_argument("task")
    ap.add_argument("--agent", default="codex")
    ap.add_argument("--model")
    ap.add_argument("--effort")
    ap.add_argument("--root", default="/tmp/agent-cu-bench")
    ap.add_argument("--port", type=int, default=8765)
    args = ap.parse_args()
    root = Path(args.root)
    run_dir = Path(args.out).resolve() / args.task
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "mcp.jsonl").unlink(missing_ok=True)
    spec = {
        "run_id": f"interview-{args.task}", "binary": args.binary, "task": args.task,
        "agent": args.agent, "model": args.model or ("gpt-6.1-terra" if args.agent == "codex" else "claude-sonnet-5-5"),
        "effort": args.effort or ("low" if args.agent == "codex" else "medium"),
        "port": args.port, "root": str(root), "codex_home": str(root / "codex-home"),
        "workdir": str(root / "work"), "timeout": 360,
    }
    task = tasks.BY_ID[args.task]
    env = run_one.session_env(root)
    run_one.http(f"http://127.0.0.1:{args.port}/api/reset", b"{}")
    app = run_one.launch_app(task, spec, env, root)
    context = ("An Alacritty terminal window is already open." if task.kind == "terminal"
               else "A Chrome window is already open on the app for this task.")
    prompt = run_one.PREAMBLE.format(context=context, prompt=task.prompt)
    agent_env = {**os.environ, "CODEX_HOME": spec["codex_home"]}
    if args.agent == "codex":
        cmd = [c for c in run_one.codex_cmd(spec, env, prompt, run_dir / "mcp.jsonl") if c != "--ephemeral"]
    else:
        cmd = [c for c in run_one.claude_cmd(spec, env, prompt, run_dir / "mcp.jsonl", run_dir / "mcp-config.json")
               if c != "--no-session-persistence"]
    started = time.monotonic()
    with open(run_dir / "agent.jsonl", "w") as out:
        proc = subprocess.Popen(cmd, stdout=out, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL,
                                env=agent_env, cwd=spec["workdir"], start_new_session=True)
        try:
            proc.wait(timeout=spec["timeout"])
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
    wall = time.monotonic() - started
    time.sleep(0.5)
    ok, detail = task.check(tasks.read_state(root / "state", task.id), root)
    run_one.stop_app(app, spec, root)
    events = [json.loads(l) for l in (run_dir / "agent.jsonl").read_text().splitlines() if l.startswith("{")]
    if args.agent == "codex":
        session = next(e["thread_id"] for e in events if e.get("type") == "thread.started")
        resume = ["codex", "exec", "resume", session, "--json", "--skip-git-repo-check",
                  "-m", spec["model"], "-c", f'model_reasoning_effort="{spec["effort"]}"', QUESTIONS]
    else:
        session = next(e["session_id"] for e in events if e.get("session_id"))
        resume = ["claude", "-p", "--resume", session, "--model", spec["model"], "--output-format", "json",
                  "--setting-sources", "", "--tools", "", QUESTIONS]
    answer = subprocess.run(resume, capture_output=True, text=True, env=agent_env, cwd=spec["workdir"],
                            stdin=subprocess.DEVNULL, timeout=300).stdout
    if args.agent == "codex":
        texts = [json.loads(l).get("item", {}).get("text", "") for l in answer.splitlines() if l.startswith("{")]
        answer = "\n".join(t for t in texts if t)
    else:
        try:
            answer = json.loads(answer).get("result", answer)
        except ValueError:
            pass
    calls = [json.loads(l) for l in (run_dir / "mcp.jsonl").read_text().splitlines()] if (run_dir / "mcp.jsonl").exists() else []
    calls = [c for c in calls if c.get("method") == "tools/call"]
    (run_dir / "interview.txt").write_text(
        f"task={args.task} ok={ok} ({detail}) wall={wall:.0f}s mcp_calls={len(calls)} "
        f"screenshots={sum(1 for c in calls if c['images'])}\n"
        f"tools: {' '.join(c['tool'] for c in calls)}\n\n{answer}\n")
    print((run_dir / "interview.txt").read_text())


if __name__ == "__main__":
    main()
