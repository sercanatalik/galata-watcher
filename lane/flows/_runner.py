"""How the lane runs one of the watcher's binaries: as a subprocess, and nothing else.

**The only module that imports ``subprocess``**, which
``scripts/check-python-flows.sh`` asserts. The leading underscore keeps it
out of ``cereyan serve``'s flow discovery.

**A job's environment is built, never inherited.** It gets exactly
:func:`job_env`, so a token or password in cereyan's own environment cannot
reach a tool. The tools need none: ``galata-notify`` reads its token from a
file of its own.

**The exit code is the tools' interface**, read by table:

    tool                 0          1                2            3
    galata-watcher-poll  alerts     Broken           BadArgument  nothing new
    galata-notify        sent       NotDelivered     BadArgument  —
    anything else        Broken

``3`` from the poll is success with nothing to say, not a failure: a quiet
minute is the normal case.
"""

from __future__ import annotations

import json
import subprocess
import tomllib
from pathlib import Path

# lane/flows/_runner.py -> lane/flows -> lane.
LANE = Path(__file__).resolve().parents[1]
SETTINGS = LANE / "lane.toml"


def _paths() -> tuple[Path, Path]:
    with SETTINGS.open("rb") as f:
        paths = tomllib.load(f)["paths"]
    return (LANE / paths["bin"]).resolve(), (LANE / paths["data"]).resolve()


# Resolved at import; the lane is restarted to pick up a changed lane.toml.
BIN, DATA = _paths()

PATH = "/usr/bin:/bin"
NO_COLOR = "1"
NOTHING = 3


class Refused(RuntimeError):
    """A tool did not do its work. Carries the tool's own output."""


class Broken(Refused):
    """Exit 1 from the poll, or a code the table does not name."""


class BadArgument(Refused):
    """Exit 2. The invocation or the configuration is wrong; repeating it will not help."""


class NotDelivered(Refused):
    """Exit 1 from galata-notify: Telegram did not take the message after retries."""


TABLES = {
    "galata-watcher-poll": {1: Broken, 2: BadArgument},
    "galata-notify": {1: NotDelivered, 2: BadArgument},
}


def job_env() -> dict[str, str]:
    """Exactly what a job receives. Nothing is inherited."""
    return {"PATH": PATH, "NO_COLOR": NO_COLOR}


def binary(name: str) -> Path:
    path = BIN / name
    if not path.exists():
        raise Broken(f"no binary at {path}: build the image, or point lane.toml's bin at target/release")
    return path


def run(tool: str, args: list[str], stdin: str | None = None) -> str:
    """Run one tool to completion and return what it printed on stdout.

    Raises a :class:`Refused` chosen by the tool's table, carrying stdout and
    stderr.
    """
    table = TABLES[tool]
    finished = subprocess.run(
        [str(binary(tool)), "--data", str(DATA), *args],
        cwd=str(LANE),
        env=job_env(),
        input=stdin,
        capture_output=True,
        text=True,
    )
    code = finished.returncode
    if code == 0 or (code == NOTHING and tool == "galata-watcher-poll"):
        return finished.stdout or ""
    said = ((finished.stdout or "") + (finished.stderr or "")).strip()
    raise table.get(code, Broken)(f"{tool} exited {code}\n{said}")


def alerts(printed: str) -> list[dict]:
    """The alerts the poll printed, one JSON object per line."""
    return [json.loads(line) for line in printed.splitlines() if line.strip()]
