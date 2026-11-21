# galata-watcher

**An always-on watcher for one Hyperliquid account that tells you on
Telegram when something happens, while the trading screen is closed.**

It sends a message when:

- a take-profit or stop-loss order fills;
- a position is liquidated, or comes within a set distance of liquidation;
- a price crosses a level you set.

It also sends a daily digest, a market update every quarter of an hour and, every half hour through the day, a PnL
summary. See [Alerts and summaries](alerts.md) for what each looks like.

It only reads the account's public data. It holds no trading key and never
places, amends or cancels an order.

## Status

0.x. It runs today on a Mac under Apple `container`. A Docker Compose
deployment for an always-on NAS is written but not yet verified.

## Part of Galata

Galata is a Rust trading framework.
[galata-desk](https://sercanatalik.github.io/galata-desk/), the desktop app,
connects to this watcher's [API](api.md): it draws the call levels on its
charts and edits the levels, stop alerts and near-liquidation distance.
Anything that can make an HTTP request can do the same.

The source is on [GitHub](https://github.com/sercanatalik/galata-watcher),
under the MIT licence.
