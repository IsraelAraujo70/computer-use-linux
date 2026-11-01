"""argo35 suite: 35 general desktop computer-use tasks (see METRIC.md, "argo35").

Reconstructs the spirit of @argofowl's 35-task Mac test (his list is not public): drag and drop,
canvas clicks, huge virtualized lists, plus context menus, shortcuts, windows/tabs, dialogs,
iframes, upload/download, tables, rich text, date pickers, hover and keyboard-only fields.

Each task is one module in this package with exactly these names:

    ID: str            task id, "a35-<kebab-name>" (state file /api/state/<ID>)
    CATEGORY: str      one of CATEGORIES below
    PAGE: str          page path under /apps/, e.g. "argo35/dnd_reorder.html"
    PROMPT: str        what the agent is asked to do (English, UI labels quoted exactly)
    def check(state: dict, root: Path) -> tuple[bool, str]
                       reads the JSON the page POSTed to /api/state/<ID> (and, for download or
                       terminal tasks, files under root); never the agent's reply
    WRONG: list[dict]  >= 3 realistic wrong or partial states; check(w, root) must reject each

optional:

    KIND: str          "web" (default) or "web+terminal" (Chrome plus an Alacritty window
                       whose working directory is root/"term")
    def setup(root: Path) -> None
                       prepares files before the app opens (root/"files" for uploads,
                       root/"term" for the terminal). Must be idempotent: it clears what it owns.

and a Playwright flow next to it, `flow_<module>.mjs`:

    export const TASK_ID = '<ID>';
    export async function flow({ page, context, base, root, assert })

that drives the happy path headlessly the way a user would (labels, roles, mouse). Canvas and drag
flows may read coordinates with page.evaluate, but must act through page.mouse / keyboard, never by
calling the page's own functions. `base` is "http://127.0.0.1:<port>/apps/argo35/", `root` the
episode root setup() wrote to; downloads go to root/"downloads" (the real episodes point Chrome's
download directory there too).
"""
from pathlib import Path

CATEGORIES = (
    "drag", "canvas", "virtual-list", "context-menu", "shortcut", "multi-window", "dialog",
    "iframe", "upload-download", "table", "rich-text", "date-picker", "hover", "keyboard-only",
)

# Default episode root; prompts that name absolute paths use it (run.py --root defaults to it too).
ROOT = Path("/tmp/agent-cu-bench")
