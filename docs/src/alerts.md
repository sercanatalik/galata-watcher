# Alerts and summaries

Every message goes to the one Telegram chat in the
[configuration](configuration.md). The watcher checks the account every
minute.

## Alerts

| When | Message |
|---|---|
| A take-profit fills | `ETH take-profit filled: sold 0.5 at 3,400` |
| A stop-loss fills | `ETH stop-loss filled: sold 0.5 at 2,900` |
| A position is liquidated | `BTC long LIQUIDATED: 0.1 at 52,000` |
| A position nears liquidation | `BTC near liquidation: mark 54,000, liquidation 52,000, 3.7% away` |
| A price crosses a call level | `BTC crossed below 58,000: weekly support` |

- **Stop alerts** (take-profit and stop-loss) have one switch for every
  ticker: `stop_alerts`.
- **Liquidation and near-liquidation alerts** can't be switched off. The
  near-liquidation distance is `near_liquidation_pct`, 2 to 50, 15 by
  default.
- **A call level** fires once, then waits to be re-armed.

An alert found after the Mac was asleep or the watcher was down is sent when
it next runs, marked `[late: happened <time>]`.

## Daily digest

Once a day at `digest.time` in `digest.zone`: each open position with its
24-hour PnL, whether it has a stop-loss and a take-profit, and its distance
from liquidation; the number of armed levels; and how many alerts went since
the last digest.

```text
Daily digest, 2026-11-20
BTC long, 24h PnL +$120: stop-loss yes, take-profit no, 8.2% from liquidation
ETH short, 24h PnL -$45: NO STOP-LOSS, take-profit yes, 21.0% from liquidation
Armed call levels: 2
Alerts since the last digest: 3
```

A missing digest is the sign that the watcher was down.

## PnL summary

On the hour and the half hour in the digest's time zone, except from 23:00 to
07:00: the account's PnL over the venue's 24h, 30D and all-time windows
(perps, spot and vaults), each open position's 24-hour PnL, and a
`Closed / other` line that makes the positions add up to the 24h figure.

A slot is sent within ten minutes of its start or not at all, and never
twice, even across a restart. `pnl_summary: false` turns it off.

## Health

If the checks fail three times in a row, the watcher sends **watcher
broken** with the last error, once. When a check succeeds again, it sends
**recovered**.
