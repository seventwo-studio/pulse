# Pulse Example Server

Reference implementation of the Pulse REST API using **Hono** + **bun:sqlite**.

This is a local-only, single-node example. The real Pulse platform will use Postgres, KV stores, and object storage for multi-tenant scale.

## Run

```bash
bun install
bun run dev       # hot-reload
bun run start     # production
```

## Environment

| Variable   | Default         | Description          |
|------------|-----------------|----------------------|
| `PULSE_DB` | `pulse.db`      | SQLite database path |
| `PORT`     | `3000`          | Server port          |

## API

See [`docs/design/rest-api.md`](../../docs/design/rest-api.md) for the full API spec.
