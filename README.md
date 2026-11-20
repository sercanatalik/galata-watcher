# galata-watcher

**An always-on watcher for one Hyperliquid account. It tells you on Telegram
when a take-profit or stop-loss fills, when a position is liquidated or
nears liquidation, and when a price crosses a level you set, even while your
Mac is asleep. Every half hour through the day it also sends a PnL summary.**

It runs in two containers from one image: for now on a Mac under Apple
`container`, later on a NAS under Docker. It only reads the account's public
data. It holds no trading key and never places, amends or cancels an order.

Galata is a Rust trading framework. galata-watcher is the trading side's
always-on half; [galata-desk](https://github.com/sercanatalik/galata-desk) is
its screen and will configure it over this tree's API.

---

## How it is built

```
  cron 1m  --> flow watch-the-account --> galata-watcher-poll --> JSON lines
                                                                       |
               emit_event("account.<kind>", alert) <-------------------+
                       |
  rule on account.*  --+--> galata-notify --> Telegram
  rule on run.failed x3 / run.completed --> galata-notify --liveness broken|ok
  cron digest time --> flow send-the-digest --> galata-watcher-poll --digest --> galata-notify
  cron :00 and :30 --> flow send-the-pnl-summary --> galata-watcher-poll --pnl --> galata-notify
```

**The PnL summary** gives the account's PnL over the venue's rolling 24h,
30D and all-time windows (perps, spot and vaults), then each open position's
24-hour PnL, computed as in the digest, and a `Closed / other` line that
makes the positions add up to the 24h figure. It is sent for each slot on
the hour and the half hour in the digest's time zone, except from 23:00 to
07:00, within ten minutes of the slot's start or not at all. The last slot
sent is kept in `pnl.json` on the data volume, so a restart never sends one
twice. To see one now, outside quiet hours and in a slot's first ten
minutes:

```sh
galata-watcher-poll --data var/data --pnl   # exit 3: no slot due
```

| Part | What it does |
|---|---|
| `crates/galata-watcher` | Library: config, state, fill classification, levels, near-liquidation, message text. All tested without a network. |
| `crates/galata-watcher-poll` | One check of the account: prints new alerts, then saves state. `--digest` and `--pnl` make the daily digest and the half-hourly PnL summary instead. |
| `crates/galata-notify` | Sends one message to Telegram and records whether it was delivered. |
| `crates/galata-watcher-api` | Reads and writes the configuration, and lists alerts and health over HTTP. |
| `lane/` | The [cereyan](https://github.com/sercanatalik/cereyan) project that schedules the checks and turns alerts into messages. It only spawns the binaries above. |
| `scripts/run-container.sh`, `launchd/` | Running on a Mac: one service under Apple `container`, and the two launchd agents that keep them running. |

## The API

`galata-watcher-api` serves four routes. Every request, `/health` included,
needs `Authorization: Bearer <token>`; without it the answer is `401` with an
empty body. Publish it only on loopback, a LAN address or a Tailscale address.

| Route | Answers |
|---|---|
| `GET /config` | `200` with the configuration and `ETag: "<version>"`; `404` (with `ETag: "0"`) before the first write |
| `PUT /config` | replaces the whole configuration; see below |
| `GET /alerts?since=<ms>` | `200` with `{"alerts": [...]}`, newest first, each with `delivered` |
| `GET /health` | `200` with `last_ok_ms`, `last_error`, `last_error_ms`, `config_version`, `address` |

**Writing.** `PUT /config` must carry `If-Match` with the version it was
based on (`"0"` for the first write).

| Status | When |
|---|---|
| `200` | saved. The body is the new configuration, and `ETag` is the new version. |
| `428` | `If-Match` is missing. |
| `412` | another write came first. `ETag` names the current version: read it again and redo the edit. |
| `422` | `{"problems": [...]}`, one line per problem, each naming its field. Nothing is saved. |

A change is used from the next check, within a minute, with no restart.

**The configuration.** `version` and each level's `generation` are set by
the API, and a writer's values for them are ignored. Leaving out
`near_liquidation_pct` (2 to 50, default 15), `network`, `digest`,
`stop_alerts` or `levels` gives the default. Liquidation and
near-liquidation alerts have no off switch, and a field that tries to add
one is refused.

<!-- example-config: a test PUTs this block verbatim -->
```json
{
  "address": "0x00000000000000000000000000000000000000aa",
  "network": "mainnet",
  "telegram_chat": "123456789",
  "near_liquidation_pct": "15",
  "digest": { "time": "07:00", "zone": "Europe/Istanbul" },
  "stop_alerts": true,
  "levels": [
    { "id": "btc-support", "market": "BTC", "price": "58000", "direction": "below", "note": "weekly support" },
    { "market": "xyz:NVDA", "price": "210", "direction": "above" }
  ]
}
```

**Levels.** A read gives each level `"state": "armed"` or `"fired"`, and
`fired_at` (milliseconds) once it has fired. To re-arm a fired level, write it
back with `"state": "armed"`. A level can't be written as `"fired"`. A level
without an `id` gets one from its market, direction and price.

**Stop alerts.** `stop_alerts` switches take-profit and stop-loss alerts on
or off for every ticker at once. When on, each TP or SL order that fills
sends one alert, with its size and average price.

## The Telegram bot

1. In Telegram, message `@BotFather`, send `/newbot`, and follow the prompts.
   It replies with the bot's token.
2. Open a chat with the new bot and send it any message.
3. Open `https://api.telegram.org/bot<TOKEN>/getUpdates` in a browser. The
   `chat.id` in the answer is your chat id.

A leaked bot token can only send messages as this bot. To replace it, send
`/revoke` to `@BotFather`.

## Running on a Mac with Apple container

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

**2. Put the bot token in place** (see the section above):

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

## Deploying on the NAS (next)

The same image, under Docker Compose on a NAS that stays on, so alerts don't
depend on the Mac. This deployment is written but not yet verified on the
NAS. The watcher runs as two containers (`lane` and `api`), with a data volume
and two secret files. It only makes outgoing calls, to
Hyperliquid and to Telegram, apart from the API port.

**1. Find the NAS's CPU.** On the NAS, `uname -m` prints `x86_64` (build
for `linux/amd64`) or `aarch64` (`linux/arm64`). Building on the NAS itself
needs no flag.

**2. Create the Telegram bot** (see above).

**3. Put the secrets on the NAS**, next to `compose.yaml`, readable only by
the image's user (uid 10001):

```sh
mkdir -p secrets
printf '%s' '<bot token>' > secrets/telegram_token
openssl rand -hex 32 > secrets/api_token      # galata-desk will need this one
sudo chown 10001 secrets/*
chmod 600 secrets/*
```

**4. Start it**, with the API on the NAS's LAN or Tailscale address:

```sh
WATCHER_API_HOST=<NAS address> docker compose up -d --build
```

**5. Write the first configuration** through the API (see the example above),
using the testnet first:

```sh
curl -X PUT http://<NAS address>:8790/config \
  -H "Authorization: Bearer $(cat secrets/api_token)" \
  -H 'If-Match: "0"' \
  -d '{"address": "0x…", "network": "testnet", "telegram_chat": "<chat id>"}'
```

Within a minute, `GET /health` should report a `last_ok_ms`. If the
digest time has already passed that day, the day's digest arrives on Telegram
within five minutes: the first sign that delivery works.

**Looking at the lane.** cereyan's UI is published nowhere. Its CLI works
from inside the container:

```sh
docker compose exec lane cereyan runs ls
```

**Updating.** Run `git pull && docker compose up -d --build`. The data volume
keeps the configuration, cursors and alert history across rebuilds.

## Development

```sh
cargo test --workspace
uv sync --project lane && (cd lane && uv run pytest)
scripts/check-python-flows.sh
```

## Licence

MIT. See [LICENSE-MIT](LICENSE-MIT).
