# Alerts and summaries

Every message goes to the one Telegram chat in the
[configuration](configuration.md). The watcher checks the account every
minute.

Messages are sent formatted: an icon for the kind of message, the event in
bold, figures in monospace and lists in aligned columns. The examples below
are the plain wording, which is also what the alert history keeps and
galata-desk shows. If Telegram ever refuses a message's formatting, it is
sent again at once as plain text.

## Alerts

| When | Message |
|---|---|
| A take-profit fills | 🟢 `ETH take-profit filled: sold 0.5 at 3,400` |
| A stop-loss fills | 🔴 `ETH stop-loss filled: sold 0.5 at 2,900` |
| A position is liquidated | 🚨 `BTC long LIQUIDATED: 0.1 at 52,000` |
| A position nears liquidation | ⚠️ `BTC near liquidation: mark 54,000, liquidation 52,000, 3.7% away` |
| A price crosses a call level | 🔔 `BTC crossed below 58,000: weekly support` |

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
(perps, spot and vaults), each open position's 24-hour PnL, a
`Closed / other` line that makes the positions add up to the 24h figure,
and the account's last three trades as a reminder of what it did. Fills of
one order count as one trade, at their average price.

```text
📊 PnL summary, 14:30
24h        +$1,235
30D        -$4,102
All-time  +$18,870
Open positions
BTC             long   +$912
ETH             short  -$140
Closed / other         +$463
Last trades
14:21         BTC  sold    0.05  at  59,300
13:02         ETH  sold       2  at   3,012
19 Nov 09:05  BTC  bought   0.5  at  58,410
```

A slot is sent within ten minutes of its start or not at all, and never
twice, even across a restart. `pnl_summary: false` turns it off.

## Health

If the checks fail three times in a row, the watcher sends **watcher
broken** with the last error, once. When a check succeeds again, it sends
**recovered**.
