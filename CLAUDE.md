# Pulse

AI-native version control system. Rust CLI client, platform-hosted server. No Git compatibility.

## Build & Run

```bash
cargo build                     # debug build
cargo build --release           # release build
cargo test                      # run all tests
```

### Example server (for local development)

```bash
cd examples/server
bun install
bun run dev                     # Hono + bun:sqlite on :3000
```

## Project Structure

```
src/
  main.rs                       # CLI entry point (clap)
  storage/                      # append-only log, structural chunker, BLAKE3, zstd
  core/                         # primitives: chunk, blob, snapshot, changeset, workspace, main
  client/                       # HTTP/WS client used by CLI
examples/
  server/                       # reference server implementation (Hono + bun:sqlite)
docs/design/                    # design documents (architecture, APIs, storage, etc.)
```

## Architecture

- **CLI**: Rust binary that talks to a remote Pulse server over HTTP
- **Storage** (client-side): append-only log with structural chunking (FastCDC fallback), BLAKE3 hashing, zstd compression
- **Server**: separate service — the example uses SQLite, the real platform will use Postgres/KV/object storage
- **Model**: single main, ephemeral workspaces, server is source of truth
- **Merge**: file-level granularity, conflicts emit `decision.needed` events

## Conventions

- Rust 2024 edition
- `thiserror` for library errors, `anyhow` for binary/CLI errors
- Return `Result` types, don't panic in library code
- `clap` derive API for CLI
- Tests use `#[tokio::test]` for async, real storage (no mocks)
