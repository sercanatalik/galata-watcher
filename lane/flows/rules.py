"""Who is told what: every alert to Telegram, and the watcher's own health.

- ``account.*``: each alert, as ``galata-notify --alert-id``, in its HTML
  form with ``--html`` when the alert carries one, else its plain text.
  ``once="never"`` because one check can find several alerts, and cereyan's default fires a
  rule once per run. ``max_per_minute`` is a flood guard only: deciding what
  is worth sending, and per-market quiet times, belong to the poll.
- ``run.failed`` of ``watch-the-account``, from the third in a row: "watcher
  broken". It fires on every failure after that; ``galata-notify`` sends it
  once and keeps a flag until a check succeeds.
- ``run.completed`` of ``watch-the-account``: "recovered", which
  ``galata-notify`` sends only while that flag is set, and otherwise returns
  without touching the network.
"""

from __future__ import annotations

from cereyan import events

from . import _runner
from ._app import APP

WATCH = "watch-the-account"


@APP.rule(on="account.*", once="never", max_per_minute=30)
def send_alert(event, run):
    alert = event["payload"]
    if alert.get("html"):
        return _runner.run("galata-notify", ["--alert-id", alert["id"], "--html"], stdin=alert["html"])
    return _runner.run("galata-notify", ["--alert-id", alert["id"]], stdin=alert["text"])


@APP.rule(on=events.run.failed, flow=WATCH, after_consecutive=3)
def watcher_broken(event, run):
    return _runner.run("galata-notify", ["--liveness", "broken"])


@APP.rule(on=events.run.completed, flow=WATCH)
def watcher_ok(event, run):
    return _runner.run("galata-notify", ["--liveness", "ok"])
