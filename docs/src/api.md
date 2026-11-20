# The API

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

The body of `GET` and `PUT /config` is described under [Configuration](configuration.md).
