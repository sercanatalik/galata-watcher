"""Send the PnL summary. Asked on the hour and the half hour.

Whether a summary is due (a slot just began, outside 23:00 to 07:00 in the
digest's zone, and none has gone for it) is decided by
``galata-watcher-poll --pnl``. Exit 3 means not due. The summary goes out as
an ``account.pnl_summary`` event, through the same rule as every alert.
"""

from __future__ import annotations

from cereyan import Cron, emit_event, flow, task

from . import _runner
from ._app import APP

SCHEDULE = Cron("*/30 * * * *", timezone="UTC")


@task(retries=0)
def summary() -> str:
    return _runner.run("galata-watcher-poll", ["--pnl"])


@flow(name="send-the-pnl-summary", schedule=SCHEDULE, max_concurrent=1, on_overlap="skip", app=APP)
def send_the_pnl_summary() -> str:
    found = _runner.alerts(summary())
    for alert in found:
        emit_event(f"account.{alert['kind']}", alert)
    return "sent" if found else "not due"
