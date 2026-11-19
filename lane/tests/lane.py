"""The lane end to end, offline, against stand-in binaries."""

from __future__ import annotations

import pytest

from flows import _runner
from flows.digest import send_the_digest
from flows.watch import watch_the_account

from conftest import alert_line


def a_scheduler_credential_never_reaches_a_job(tools, monkeypatch):
    monkeypatch.setenv("TELEGRAM_TOKEN", "should-not-travel")
    monkeypatch.setenv("CEREYAN_TOKEN", "also-not")
    tools.acts("galata-watcher-poll", (3, ""))
    watch_the_account()
    [call] = tools.calls("galata-watcher-poll")
    env = {k: v for k, v in call["env"].items() if k not in {"PWD", "SHLVL", "_", "OLDPWD"}}
    assert env == {"PATH": "/usr/bin:/bin", "NO_COLOR": "1"}


def the_poll_is_pointed_at_the_data_volume(tools):
    tools.acts("galata-watcher-poll", (3, ""))
    watch_the_account()
    [call] = tools.calls("galata-watcher-poll")
    assert call["argv"] == ["--data", str(_runner.DATA)]


def two_alerts_become_two_events_and_two_messages(tools):
    tools.acts("galata-watcher-poll", (0, f"{alert_line('sl-1')}|{alert_line('tp-2', 'take_profit')}"))
    assert watch_the_account() == "2 alert(s)"
    sent = [c for c in tools.calls("galata-notify") if "--alert-id" in c["argv"]]
    assert [c["argv"][c["argv"].index("--alert-id") + 1] for c in sent] == ["sl-1", "tp-2"]
    assert [c["stdin"] for c in sent] == ["text of sl-1", "text of tp-2"]
    assert all(c["argv"][:2] == ["--data", str(_runner.DATA)] for c in sent)


def a_quiet_minute_emits_nothing_and_succeeds(tools):
    tools.acts("galata-watcher-poll", (3, ""))
    assert watch_the_account() == "0 alert(s)"
    assert not [c for c in tools.calls("galata-notify") if "--alert-id" in c["argv"]]


def a_successful_check_asks_notify_whether_to_say_recovered(tools):
    tools.acts("galata-watcher-poll", (3, ""))
    watch_the_account()
    assert [c["argv"][2:] for c in tools.calls("galata-notify")] == [["--liveness", "ok"]]


def a_broken_check_fails_the_run_and_asks_notify_to_say_broken(tools):
    tools.acts("galata-watcher-poll", (1, "venue: no connection"))
    with pytest.raises(_runner.Broken, match="exited 1"):
        watch_the_account()
    # Offline, cereyan does not apply `after_consecutive`; the server does
    # (see the rule spec test below, and the server test).
    assert [c["argv"][2:] for c in tools.calls("galata-notify")] == [["--liveness", "broken"]]


def a_bad_configuration_is_a_bad_argument(tools):
    tools.acts("galata-watcher-poll", (2, ""))
    with pytest.raises(_runner.BadArgument):
        watch_the_account()


def an_unnamed_exit_code_is_broken(tools):
    tools.acts("galata-watcher-poll", (101, ""))
    with pytest.raises(_runner.Broken, match="exited 101"):
        watch_the_account()


def a_missing_binary_names_the_build(tools):
    (tools.bin / "galata-watcher-poll").unlink(missing_ok=True)
    with pytest.raises(_runner.Broken, match="no binary"):
        watch_the_account()


def the_digest_is_sent_through_the_alert_rule_when_due(tools):
    tools.acts("galata-watcher-poll", (0, alert_line("digest-2026-11-19", "digest", "Daily digest")))
    assert send_the_digest() == "sent"
    [call] = tools.calls("galata-watcher-poll")
    assert call["argv"] == ["--data", str(_runner.DATA), "--digest"]
    sent = [c for c in tools.calls("galata-notify") if "--alert-id" in c["argv"]]
    assert [c["stdin"] for c in sent] == ["Daily digest"]


def a_digest_not_yet_due_sends_nothing(tools):
    tools.acts("galata-watcher-poll", (3, ""))
    assert send_the_digest() == "not due"
    assert tools.calls("galata-notify") == []


def the_rules_carry_the_guards_the_design_names():
    from cereyan.rules import all_rules

    specs = {r.name: r.spec() for r in all_rules() if r.module.startswith("flows")}
    assert specs["send_alert"]["when"]["events"] == ["account.*"]
    assert specs["send_alert"]["once"] == "never"
    assert specs["watcher_broken"]["when"]["events"] == ["run.failed"]
    assert specs["watcher_broken"]["when"]["flows"] == ["watch-the-account"]
    assert specs["watcher_broken"]["after_consecutive"] == 3
    assert specs["watcher_ok"]["when"]["events"] == ["run.completed"]
    assert specs["watcher_ok"]["when"]["flows"] == ["watch-the-account"]
