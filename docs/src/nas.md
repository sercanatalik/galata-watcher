# On a NAS

The same image, under Docker Compose on a NAS that stays on, so alerts don't
depend on the Mac. This deployment is written but not yet verified on the
NAS. The watcher runs as two containers (`lane` and `api`), with a data volume
and two secret files. It only makes outgoing calls, to
Hyperliquid and to Telegram, apart from the API port.

**1. Find the NAS's CPU.** On the NAS, `uname -m` prints `x86_64` (build
for `linux/amd64`) or `aarch64` (`linux/arm64`). Building on the NAS itself
needs no flag.

**2. Create the Telegram bot** (see [Quick start](quick-start.md#1-create-a-telegram-bot)).

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

**5. Write the first configuration** through the API (see [Configuration](configuration.md)),
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
