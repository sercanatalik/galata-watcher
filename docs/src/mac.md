# On a Mac

While galata-watcher is being developed, it runs on a Mac under
[Apple container](https://github.com/apple/container) (1.7 or later), on
Apple silicon. Two launchd agents, `com.galata.watcher.lane` and
`com.galata.watcher.api`, each run `scripts/run-container.sh`, which starts
one container in the foreground. launchd restarts it if it exits, and starts
it again at login.

| | |
|---|---|
| Data (configuration, cursors, alert history, cereyan's store) | `var/data/` |
| Logs | `var/logs/lane.log`, `var/logs/api.log` |
| Secrets, mode 0600, each mounted only into its own container | `var/secrets/lane/telegram_token`, `var/secrets/api/api_token` |
| API | `http://127.0.0.1:8790`, this Mac only |

**It only alerts while the Mac is awake.** A Mac that sleeps or is off sends
nothing. When it wakes, the watcher sends what it missed, each alert marked
late with the time it happened, and a missing 07:00 digest is the sign that
it was down. Check the Mac's sleep settings with `pmset -g`. The NAS
deployment below doesn't have this limit.

**1. Start Apple container once by hand**, so it can install its kernel:
`container system start`.

**2. Put the bot token in place** (see [Quick start](quick-start.md#1-create-a-telegram-bot)):

```sh
mkdir -p var/secrets/lane && chmod 700 var/secrets var/secrets/lane
(umask 077 && printf '%s' '<bot token>' > var/secrets/lane/telegram_token)
```

**3. Install.** This builds the image (several minutes the first time),
creates `var/secrets/api/api_token` if there is none, checks both services,
and loads both agents:

```sh
launchd/install.sh
```

If a precondition is missing, it says which one and loads nothing.
`scripts/run-container.sh --check lane` (or `api`) checks one service the
same way.

**4. Write the first configuration** through the API, using the testnet first:

```sh
curl -X PUT http://127.0.0.1:8790/config \
  -H "Authorization: Bearer $(cat var/secrets/api/api_token)" \
  -H 'If-Match: "0"' \
  -d '{"address": "0x…", "network": "testnet", "telegram_chat": "<chat id>"}'
```

If a configuration was already written, `If-Match` must carry its version:
`GET /config` returns it in `ETag`. Within a minute, `GET /health` should
report a `last_ok_ms`. If the digest time has already passed that day, the
day's digest arrives on Telegram within five minutes: the first sign that
delivery works.

**Updating.** Run `git pull && launchd/install.sh`. The image is rebuilt and
both agents restart on it. `var/` is kept.

**Replacing a secret.** Overwrite the file, then restart its agent:
`launchctl kickstart -k gui/$(id -u)/com.galata.watcher.lane` (or `.api`).

**Looking at the lane.** cereyan's UI listens only inside the lane's
container. Its CLI works from there:

```sh
container exec gw-lane cereyan runs ls
container exec gw-lane cereyan runs ls --state Failed
```

**Removing it.** `launchd/install.sh --uninstall` stops and removes both
agents and leaves `var/` alone.
