"""Send the daily digest. Asked every five minutes.

Whether a digest is due (the configured local time has passed today, and
none has gone today) is decided by ``galata-watcher-poll --digest``, which
reads the time and zone from the configuration. Exit 3 means not yet. The
digest goes out as an ``account.digest`` event, through the same rule as
every alert.
"""

from __future__ import annotations

from cereyan import Cron, emit_event, flow, task

from . import _runner
from ._app import APP

SCHEDULE = Cron("*/5 * * * *", timezone="UTC")


@task(retries=0)
def digest() -> str:
    return _runner.run("galata-watcher-poll", ["--digest"])


@flow(name="send-the-digest", schedule=SCHEDULE, max_concurrent=1, on_overlap="skip", app=APP)
def send_the_digest() -> str:
    found = _runner.alerts(digest())
    for alert in found:
        emit_event(f"account.{alert['kind']}", alert)
    return "sent" if found else "not due"
