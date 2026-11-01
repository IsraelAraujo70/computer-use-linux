"""Registry of the argo35 suite (module contract in argo35_tasks/__init__.py).

The train/holdout split is drawn once, by seed, from the sorted module names; it was fixed and
committed before the first hill-climb iteration (METRIC.md, "argo35"). Holdout tasks run only in
the final measurement.
"""
import importlib
import random

from argo35_tasks import CATEGORIES  # noqa: F401  (re-exported for the selftest)
from tasks import Task

MODULES = [
    # drag
    "dnd_reorder", "dnd_file_folder", "dnd_slider", "dnd_resize_panel", "dnd_kanban_pointer", "dnd_upload_zone",
    # canvas
    "canvas_draw_rect", "canvas_map_pin", "canvas_connect", "canvas_seats", "canvas_chart", "canvas_signature",
    # huge virtualized lists
    "vlist_10k", "vlist_logs", "vlist_grid", "vlist_infinite", "vlist_listbox",
    # the rest of general desktop use
    "ctx_rename", "ctx_submenu", "kbd_gmail", "kbd_palette", "popup_oauth", "tabs_compare", "term_code",
    "native_dialogs", "dialog_nested", "iframe_nested", "iframe_scroll_terms", "download_csv",
    "upload_hidden_input", "table_sort_edit", "rte_format", "datepicker_range", "hover_tooltip", "kbd_only_otp",
]
assert len(MODULES) == 35 and len(set(MODULES)) == 35

SPLIT_SEED = 20261101
HOLDOUT_SIZE = 10
HOLDOUT_MODULES = sorted(random.Random(SPLIT_SEED).sample(sorted(MODULES), HOLDOUT_SIZE))
TRAIN_MODULES = [m for m in MODULES if m not in HOLDOUT_MODULES]


def module(name: str):
    return importlib.import_module(f"argo35_tasks.{name}")


def _task(name: str) -> Task:
    m = module(name)
    return Task(m.ID, getattr(m, "KIND", "web"), m.PAGE, m.PROMPT, m.check, getattr(m, "setup", None))


def tasks(which: str = "all") -> list[Task]:
    """Tasks of the whole suite ("all"), "train" or "holdout", in MODULES order.

    Modules import lazily so one module can be selftested before the others exist."""
    names = {"all": MODULES, "train": TRAIN_MODULES, "holdout": HOLDOUT_MODULES}[which]
    return [_task(n) for n in MODULES if n in names]


def category(task_id: str) -> str:
    return {module(n).ID: module(n).CATEGORY for n in MODULES}[task_id]


def wrong_cases() -> dict[str, list[dict]]:
    return {module(n).ID: module(n).WRONG for n in MODULES}
