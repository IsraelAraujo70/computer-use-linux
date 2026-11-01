#!/usr/bin/env python3
"""Feed selftest_long.mjs's STATE lines into the long checkers: each must pass its happy
path, and must fail on an empty state and on every WRONG case of its module."""
import json
import sys
from pathlib import Path

import tasks_long

nowhere = Path("/nonexistent")
by_id = {t.id: t for t in tasks_long.LONG_TASKS}
wrong = tasks_long.wrong_cases()
bad = 0
seen = set()
for line in Path(sys.argv[1]).read_text().splitlines():
    if line.startswith("FLOW ") and " ERROR " in line:
        print(line)
        bad += 1
    if not line.startswith("STATE "):
        continue
    _, task_id, payload = line.split(" ", 2)
    seen.add(task_id)
    task = by_id[task_id]
    ok, detail = task.check(json.loads(payload), nowhere)
    empty_ok, _ = task.check({}, nowhere)
    accepted = [w for w in wrong[task_id] if task.check(w, nowhere)[0]]
    print(f"{task_id:22} happy={'PASS' if ok else 'FAIL ' + detail}  empty={'WRONGLY PASSES' if empty_ok else 'fails (good)'}"
          f"  wrong={len(wrong[task_id]) - len(accepted)}/{len(wrong[task_id])} rejected")
    for w in accepted:
        print("   wrongly accepted:", json.dumps(w, ensure_ascii=False))
    bad += (not ok) + empty_ok + len(accepted) + (len(wrong[task_id]) < 3)
missing = set(by_id) - seen
if missing:
    print("no selftest flow for:", sorted(missing))
    bad += 1
sys.exit(1 if bad else 0)
