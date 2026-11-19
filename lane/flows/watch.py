"""Check the account. Every minute.

Each line ``galata-watcher-poll`` prints becomes one ``account.<kind>``
event carrying the alert; ``rules.py`` sends them. A quiet minute (exit 3)
completes with nothing emitted. A failed check fails the run, and three in a
row tell the trader the watcher is broken.
"""

from __future__ import annotations

from cereyan import Cron, emit_event, flow, task

from . import _runner
from ._app import APP

SCHEDULE = Cron("* * * * *", timezone="UTC")


@task(retries=0)
def check() -> str:
    return _runner.run("galata-watcher-poll", [])


@flow(name="watch-the-account", schedule=SCHEDULE, max_concurrent=1, on_overlap="skip", app=APP)
def watch_the_account() -> str:
    found = _runner.alerts(check())
    for alert in found:
        emit_event(f"account.{alert['kind']}", alert)
    return f"{len(found)} alert(s)"
