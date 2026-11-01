"""Registry of the long suite (see long_tasks/__init__.py for the module contract)."""
import importlib

from tasks import Task

MODULES = ["onboarding", "expenses", "triage", "crm", "checkout"]


def _task(name: str) -> Task:
    module = importlib.import_module(f"long_tasks.{name}")
    return Task(module.ID, "web", module.PAGE, module.PROMPT, module.check)


def wrong_cases() -> dict[str, list[dict]]:
    return {importlib.import_module(f"long_tasks.{n}").ID: importlib.import_module(f"long_tasks.{n}").WRONG
            for n in MODULES}


LONG_TASKS = [_task(name) for name in MODULES]
