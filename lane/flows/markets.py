"""Send the market update. Asked every quarter of an hour.

Whether an update is due (a slot just began, outside 23:00 to 07:00 in the
digest's zone, a market is watched, and none has gone for the slot) is
decided by ``galata-watcher-poll --markets``. Exit 3 means not due. The
update goes out as an ``account.market_update`` event, through the same rule
as every alert.
"""

from __future__ import annotations

from cereyan import Cron, emit_event, flow, task

from . import _runner
from ._app import APP

SCHEDULE = Cron("*/15 * * * *", timezone="UTC")


@task(retries=0)
def update() -> str:
    return _runner.run("galata-watcher-poll", ["--markets"])


@flow(name="send-the-market-update", schedule=SCHEDULE, max_concurrent=1, on_overlap="skip", app=APP)
def send_the_market_update() -> str:
    found = _runner.alerts(update())
    for alert in found:
        emit_event(f"account.{alert['kind']}", alert)
    return "sent" if found else "not due"
