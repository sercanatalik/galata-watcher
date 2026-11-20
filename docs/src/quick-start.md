# Quick start

This sets the watcher up on an Apple silicon Mac. To run it on a NAS
instead, see [On a NAS](nas.md).

## 1. Create a Telegram bot

1. In Telegram, message `@BotFather`, send `/newbot`, and follow the prompts.
   It replies with the bot's token.
2. Open a chat with the new bot and send it any message.
3. Open `https://api.telegram.org/bot<TOKEN>/getUpdates` in a browser. The
   `chat.id` in the answer is your chat id.

A leaked bot token can only send messages as this bot. To replace it, send
`/revoke` to `@BotFather`.

## 2. Install

You need [Apple container](https://github.com/apple/container) 1.7 or later.

```sh
git clone https://github.com/sercanatalik/galata-watcher
cd galata-watcher
container system start                      # once, so it can install its kernel
mkdir -p var/secrets/lane && chmod 700 var/secrets var/secrets/lane
(umask 077 && printf '%s' '<bot token>' > var/secrets/lane/telegram_token)
launchd/install.sh
```

`launchd/install.sh` builds the image (several minutes the first time),
creates an API token in `var/secrets/api/api_token`, checks both services and
loads them. If something is missing, it says what and loads nothing.

## 3. Write the first configuration

Use the testnet first:

```sh
curl -X PUT http://127.0.0.1:8790/config \
  -H "Authorization: Bearer $(cat var/secrets/api/api_token)" \
  -H 'If-Match: "0"' \
  -d '{"address": "0x…", "network": "testnet", "telegram_chat": "<chat id>"}'
```

Within a minute, `GET /health` should report a `last_ok_ms`. If the digest
time has already passed that day, the day's digest arrives on Telegram within
five minutes: the first sign that delivery works.

From here, [galata-desk](https://sercanatalik.github.io/galata-desk/watcher.html)
can edit the configuration: put `http://127.0.0.1:8790` and the API token in
its Settings.
