#!/usr/bin/env python3
"""Selftest for the argo35 checkers: every task's happy path (its Playwright flow) must pass its
checker, and an empty state plus every WRONG case must fail. No page may throw.

Usage (always through the shared lock):
  /home/israel/.cache/t3-testdata/heavy.sh python3 check_selftest_argo35.py [module ...]
With no modules it runs all 35. Starts its own server.py on a free port with a throwaway state dir.
"""
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import tasks_argo35  # noqa: E402

PLAYWRIGHT_CORE = os.environ.get(
    "PLAYWRIGHT_CORE",
    "/home/israel/Projects/opensource/t3code-x11/node_modules/.pnpm/playwright-core@1.60.0/node_modules/playwright-core")
CHROME = os.environ.get("CHROME", str(sorted(Path.home().glob(".cache/ms-playwright/chromium-*/chrome-linux64/chrome"))[-1]))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main() -> int:
    names = sys.argv[1:] or tasks_argo35.MODULES
    unknown = [n for n in names if n not in tasks_argo35.MODULES]
    if unknown:
        print("unknown modules:", unknown)
        return 2
    work = Path(tempfile.mkdtemp(prefix="argo35-selftest-"))
    root = work / "root"
    root.mkdir()
    port = free_port()
    server = subprocess.Popen([sys.executable, str(HERE / "server.py"), "--port", str(port),
                               "--state-dir", str(work / "state")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(50):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1).read()
                break
            except OSError:
                time.sleep(0.1)
        mods = {n: tasks_argo35.module(n) for n in names}
        for m in mods.values():
            if hasattr(m, "setup"):
                m.setup(root)
        out = subprocess.run(["node", str(HERE / "selftest_argo35.mjs"), str(port), *names], capture_output=True, text=True,
                             env={**os.environ, "PLAYWRIGHT_CORE": PLAYWRIGHT_CORE, "CHROME": CHROME, "ARGO_ROOT": str(root)},
                             timeout=60 * len(names) + 60)
    finally:
        server.terminate()
    if out.returncode != 0:
        print(out.stdout, out.stderr)
        return 1
    bad, seen = 0, set()
    for line in out.stdout.splitlines():
        if line.startswith("FLOW ") and " ERROR " in line:
            print(line)
            bad += 1
        if line.startswith("PAGEERRORS "):
            errors = json.loads(line.split(" ", 1)[1])
            if errors:
                print("page errors:", errors)
                bad += 1
        if not line.startswith("STATE "):
            continue
        _, name, task_id, payload = line.split(" ", 3)
        m = mods[name]
        seen.add(name)
        problems = []
        if task_id != m.ID or not task_id.startswith("a35-"):
            problems.append(f"flow TASK_ID {task_id!r} != module ID {m.ID!r}")
        if m.CATEGORY not in tasks_argo35.CATEGORIES:
            problems.append(f"unknown CATEGORY {m.CATEGORY!r}")
        if m.PAGE != f"argo35/{name}.html" or not (HERE / "apps" / m.PAGE).exists():
            problems.append(f"PAGE {m.PAGE!r} must be argo35/{name}.html and exist")
        ok, detail = m.check(json.loads(payload), root)
        empty_ok, _ = m.check({}, root)
        accepted = [w for w in m.WRONG if m.check(w, root)[0]]
        if len(m.WRONG) < 3:
            problems.append(f"only {len(m.WRONG)} WRONG cases (want >= 3)")
        print(f"{name:22} happy={'PASS' if ok else 'FAIL ' + detail}  empty={'WRONGLY PASSES' if empty_ok else 'fails (good)'}"
              f"  wrong={len(m.WRONG) - len(accepted)}/{len(m.WRONG)} rejected")
        for w in accepted:
            print("   wrongly accepted:", json.dumps(w, ensure_ascii=False)[:300])
        for p in problems:
            print("   problem:", p)
        bad += (not ok) + empty_ok + len(accepted) + len(problems)
    missing = set(names) - seen
    if missing:
        print("no STATE line for:", sorted(missing))
        bad += 1
    print("SELFTEST", "OK" if not bad else f"FAILED ({bad} problems)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
