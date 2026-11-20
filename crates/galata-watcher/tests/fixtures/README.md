# Fixtures

Recorded 2026-11-19 from Hyperliquid mainnet's public info endpoint, with
read-only queries of public accounts. No key was used and no order was placed.
Account addresses are replaced: the watched account is
`0x00000000000000000000000000000000000000aa`, any other is `…bb`. Transaction
hashes are replaced by numbered placeholders (`0x…01`, `0x…02`, …), keeping
fills that shared a hash sharing one.

| File | What it holds |
|---|---|
| `historicalOrders_triggers.json` | Every status row of six triggered orders (`Stop Market`, `Stop Limit`, `Take Profit Market`, `Take Profit Limit`; position-attached and standalone), each `open` → `triggered` → `filled`, plus the rows of three ordinary limit orders. |
| `userFillsByTime_triggers.json` | The fills of those orders. A triggered order's fills carry its own `oid`, between 1 and 7 fills each. Also three ordinary fills. |
| `userFills_liquidation.json` | Six liquidation fills from three accounts (`dir` stays `Close Long`; a `liquidation` object marks them), all with `liquidatedUser` set to the watched account. The last row is **synthetic**: a copy of the first with `liquidatedUser` set to another account, `tid` 1, `dir` `Open Long`, standing for the liquidator's side of a fill. |
| `clearinghouseState.json` | Copied from galata-desk-core's fixtures: a perps state with positions and `liquidationPx`. |
| `candleSnapshot_BTC.json` | Copied from galata-desk-core's fixtures: BTC candles. |
| `portfolio.json` | Copied from galata-desk-core's fixtures: an account's PnL and value history for `day`, `week`, `month`, `allTime` and the `perp` versions. |
