#!/usr/bin/env python3
"""Feed selftest.mjs's STATE lines into the checkers: each must pass, and each
must fail on an empty state (the untouched page)."""
import json
import sys
from pathlib import Path

import tasks

bad = 0
seen = set()
for line in Path(sys.argv[1]).read_text().splitlines():
    if not line.startswith("STATE "):
        continue
    _, task_id, payload = line.split(" ", 2)
    seen.add(task_id)
    task = tasks.BY_ID[task_id]
    ok, detail = task.check(json.loads(payload), Path("/nonexistent"))
    empty_ok, _ = task.check({}, Path("/nonexistent"))
    print(f"{task_id:18} happy={'PASS' if ok else 'FAIL ' + detail}  empty={'WRONGLY PASSES' if empty_ok else 'fails (good)'}")
    bad += (not ok) + empty_ok
missing = {t.id for t in tasks.TASKS if t.kind == "web"} - seen
if missing:
    print("no selftest flow for:", sorted(missing))
    bad += 1
sys.exit(1 if bad else 0)
