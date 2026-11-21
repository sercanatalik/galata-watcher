"""Stand-in binaries, so the lane runs end to end without the watcher.

Each stand-in is a shell script at ``<bin>/<tool>`` that logs its argv, its
stdin and its whole environment, prints the next canned output, and exits
with the next code from a per-tool queue (the last one repeats). Flows run as
plain functions against a temporary ``CEREYAN_HOME``: cereyan's own offline
mode, with the lane's code rules registered in that store.
"""

from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from flows import _runner

STAND_IN = """#!/bin/sh
here=$(dirname "$0")
n=$(ls "$here" | grep -c "^{tool}\\.call\\..*\\.argv$")
call="$here/{tool}.call.$n"
printf '%s\\n' "$*" > "$call.argv"
cat > "$call.stdin"
env > "$call.env"
next() {{
  q="$here/{tool}.$1"
  v=$(head -n 1 "$q")
  if [ "$(wc -l < "$q")" -gt 1 ]; then tail -n +2 "$q" > "$q.next" && mv "$q.next" "$q"; fi
  printf '%s' "$v"
}}
out=$(next outs)
[ -n "$out" ] && [ "$out" != "-" ] && printf '%s\\n' "$out" | tr '|' '\\n'
exit "$(next codes)"
"""


class Tools:
    """The stand-in bin directory, and what each tool was asked."""

    def __init__(self, bin: Path):
        self.bin = bin

    def acts(self, tool: str, *steps: tuple[int, str]) -> None:
        """Each call takes the next (exit code, stdout) step; ``|`` separates lines."""
        script = self.bin / tool
        script.write_text(STAND_IN.format(tool=tool))
        script.chmod(script.stat().st_mode | stat.S_IXUSR)
        (self.bin / f"{tool}.codes").write_text("".join(f"{c}\n" for c, _ in steps))
        (self.bin / f"{tool}.outs").write_text("".join(f"{o or '-'}\n" for _, o in steps))

    def calls(self, tool: str) -> list[dict]:
        out = []
        n = 0
        while (self.bin / f"{tool}.call.{n}.argv").exists():
            base = self.bin / f"{tool}.call.{n}"
            env = dict(
                line.split("=", 1)
                for line in Path(f"{base}.env").read_text().splitlines()
                if "=" in line
            )
            out.append({
                "argv": Path(f"{base}.argv").read_text().split(),
                "stdin": Path(f"{base}.stdin").read_text(),
                "env": env,
            })
            n += 1
        return out


def alert_line(id: str, kind: str = "stop_loss", text: str | None = None, html: str | None = None) -> str:
    alert = {"id": id, "kind": kind, "market": "BTC", "event_ms": 1, "detected_ms": 1,
             "late": False, "text": text or f"text of {id}"}
    if html is not None:
        alert["html"] = html
    return json.dumps(alert)


@pytest.fixture(autouse=True)
def cereyan_home(tmp_path, monkeypatch):
    monkeypatch.setenv("CEREYAN_HOME", str(tmp_path / "home"))
    from cereyan import engine
    from cereyan.rules import register_with_store

    import flows.rules  # noqa: F401  (registers the code rules)

    register_with_store(engine.get_store())
    from cereyan import rules

    rules._guards.clear()


@pytest.fixture
def tools(tmp_path, monkeypatch) -> Tools:
    bin = tmp_path / "bin"
    bin.mkdir()
    data = tmp_path / "data"
    data.mkdir()
    monkeypatch.setattr(_runner, "BIN", bin)
    monkeypatch.setattr(_runner, "DATA", data)
    t = Tools(bin)
    t.acts("galata-notify", (0, ""))
    return t
