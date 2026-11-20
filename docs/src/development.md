# Development

```sh
cargo test --workspace
uv sync --project lane && (cd lane && uv run pytest)
scripts/check-python-flows.sh
```

The library's tests run without a network, against venue answers recorded
from Hyperliquid mainnet's public endpoint, with account addresses and
transaction hashes replaced (see `crates/galata-watcher/tests/fixtures/`).

A test PUTs the README's example configuration verbatim, so that example
always matches what the API accepts.

## This site

The docs are an [mdBook](https://rust-lang.github.io/mdBook/) in `docs/`.
Preview with `mdbook serve docs`. Pushing to `main` publishes them through
GitHub Actions.
