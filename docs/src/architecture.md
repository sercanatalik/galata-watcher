# How it is built

```
  cron 1m  --> flow watch-the-account --> galata-watcher-poll --> JSON lines
                                                                       |
               emit_event("account.<kind>", alert) <-------------------+
                       |
  rule on account.*  --+--> galata-notify --> Telegram
  rule on run.failed x3 / run.completed --> galata-notify --liveness broken|ok
  cron digest time --> flow send-the-digest --> galata-watcher-poll --digest --> galata-notify
  cron :00 and :30 --> flow send-the-pnl-summary --> galata-watcher-poll --pnl --> galata-notify
  cron every 15m --> flow send-the-market-update --> galata-watcher-poll --markets --> galata-notify
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

**The market update** lists each market in the configuration's `watchlist`
with its last price (the mid, or the mark where there is no mid) and its
change over 24 hours, at :00, :15, :30 and :45, with the PnL summary's quiet
hours, grace and once-per-slot record (`markets.json`). It asks the venue for
market figures only, one question per dex.

| Part | What it does |
|---|---|
| `crates/galata-watcher` | Library: config, state, fill classification, levels, near-liquidation, message text. All tested without a network. |
| `crates/galata-watcher-poll` | One check of the account: prints new alerts, then saves state. `--digest`, `--pnl` and `--markets` make the daily digest, the half-hourly PnL summary and the quarter-hourly market update instead. |
| `crates/galata-notify` | Sends one message to Telegram and records whether it was delivered. |
| `crates/galata-watcher-api` | Reads and writes the configuration, and lists alerts and health over HTTP. |
| `lane/` | The [cereyan](https://github.com/sercanatalik/cereyan) project that schedules the checks and turns alerts into messages. It only spawns the binaries above. |
| `scripts/run-container.sh`, `launchd/` | Running on a Mac: one service under Apple `container`, and the two launchd agents that keep them running. |
