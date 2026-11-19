"""The liveness rules under a real ``cereyan serve``.

Offline, cereyan does not apply ``after_consecutive``; only the server does.
So this starts one on a copy of the lane with stand-in binaries, pauses its
schedules, submits four failing checks and two good ones, and reads what
``galata-notify`` was asked. About fifteen seconds.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import time
from pathlib import Path

LANE = Path(__file__).resolve().parents[1]

POLL = """#!/bin/sh
here=$(dirname "$0"); n=$(cat "$here/poll.n" 2>/dev/null || echo 0); echo $((n+1)) > "$here/poll.n"
if [ "$n" -lt 4 ]; then echo "venue: down" >&2; exit 1; fi
exit 3
"""
NOTIFY = """#!/bin/sh
echo "$*" >> "$(dirname "$0")/notify.log"
"""


def three_failures_in_a_row_ask_for_broken_and_a_success_for_ok(tmp_path):
    proj, bin, data, home = (tmp_path / d for d in ("proj", "bin", "data", "home"))
    for d in (bin, data, home):
        d.mkdir()
    shutil.copytree(LANE / "flows", proj / "flows", ignore=shutil.ignore_patterns("__pycache__"))
    (proj / "lane.toml").write_text(f'[paths]\nbin = "{bin}"\ndata = "{data}"\n')
    for name, body in (("galata-watcher-poll", POLL), ("galata-notify", NOTIFY)):
        (bin / name).write_text(body)
        (bin / name).chmod(0o755)

    env = {"CEREYAN_HOME": str(home), "PATH": "/usr/bin:/bin"}
    server = subprocess.Popen(
        [sys.executable, "-m", "cereyan", "serve", ".", "--port", "0", "--no-open"],
        cwd=proj, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        from cereyan import client

        c = None
        for _ in range(80):
            try:
                c = client.find_server(str(home))
                if c and c.health():
                    break
            except Exception:  # noqa: BLE001  (still starting)
                pass
            time.sleep(0.25)
        assert c is not None, "the server did not start"
        c.pause_scheduler("test")

        outcomes = []
        for _ in range(6):
            run = c.submit("galata-watcher", "watch-the-account")
            for _ in range(120):
                state = c.get_run(run["id"])["state"]["type"]
                if state in ("Completed", "Failed", "Crashed"):
                    break
                time.sleep(0.1)
            outcomes.append(state)
            time.sleep(1.0)  # let the run's rules fire before the next run
        time.sleep(1.0)
    finally:
        server.terminate()
        server.wait(timeout=20)

    assert outcomes == ["Failed"] * 4 + ["Completed"] * 2
    asked = [line.split()[-1] for line in (bin / "notify.log").read_text().splitlines()]
    assert asked == ["broken", "broken", "ok", "ok"], "broken from the third failure on; ok after each success"
