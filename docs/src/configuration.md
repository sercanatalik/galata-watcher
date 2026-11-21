# Configuration

The watcher keeps one configuration, read and written through the [API](api.md).

**The configuration.** `version` and each level's `generation` are set by
the API, and a writer's values for them are ignored. Leaving out
`near_liquidation_pct` (2 to 50, default 15), `network`, `digest`,
`stop_alerts`, `pnl_summary`, `market_update`, `watchlist` or `levels` gives
the default. Liquidation and
near-liquidation alerts have no off switch, and a field that tries to add
one is refused.

```json
{
  "address": "0x00000000000000000000000000000000000000aa",
  "network": "mainnet",
  "telegram_chat": "123456789",
  "near_liquidation_pct": "15",
  "digest": { "time": "07:00", "zone": "Europe/Istanbul" },
  "stop_alerts": true,
  "pnl_summary": true,
  "market_update": true,
  "watchlist": ["BTC", "xyz:NVDA"],
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

**PnL summary.** `pnl_summary` switches the half-hourly PnL summary on or
off; it is on when left out. While it is off no summary is sent, and a slot
that passed while it was off is not sent later.

**Market update.** `watchlist` names the markets the quarter-hourly market
update lists, in order, at most 40: `BTC`, or `dex:TICKER` on a builder dex.
galata-desk writes its own watchlist here whenever it changes. `market_update`
switches the update on or off; it is on when left out, and nothing is sent
while the watchlist is empty.
