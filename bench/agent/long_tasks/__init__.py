"""Long task suite: realistic tasks of 20+ UI actions each (secondary metric, see METRIC.md).

Each task is one module in this package with exactly these names:

    ID: str            task id, e.g. "long-onboarding"
    PAGE: str          page path under /apps/, e.g. "long/onboarding.html"
    PROMPT: str        what the agent is asked to do (English, values quoted exactly)
    def check(state: dict, root: Path) -> tuple[bool, str]
                       reads the JSON the page POSTed to /api/state/<ID>; never the agent's reply
    WRONG: list[dict]  realistic wrong or partial outcomes; check() must reject each

and a Playwright flow next to it, `flow_<module>.mjs`, exporting
`export async function flow({ page, base, assert })` that drives the happy path with
the visible UI (labels, roles, buttons), the way a user would.
"""
