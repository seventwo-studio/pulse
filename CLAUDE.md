# Seven (codename: Pulse)

AI-native version control system. Single Rust binary. No Git compatibility.

## Build & Run

```bash
cargo build                     # debug build
cargo build --release           # release build
cargo test                      # run all tests
cargo run -- server start       # run server
cargo run -- server start --local  # local mode
```

## Project Structure

```
src/
  main.rs                       # CLI entry point (clap)
  server/                       # REST API (axum) + WebSocket feed
  storage/                      # append-only log, structural chunker, BLAKE3, zstd
  core/                         # primitives: chunk, blob, snapshot, changeset, workspace, trunk
  client/                       # HTTP/WS client used by CLI
docs/design/                    # design documents (architecture, APIs, storage, etc.)
```

## Architecture

- **Storage**: custom append-only log with structural chunking (FastCDC fallback), BLAKE3 hashing, zstd compression
- **API**: REST for commands/queries, WebSocket for real-time events
- **Model**: single trunk, ephemeral workspaces, server is source of truth
- **Merge**: file-level granularity, conflicts emit `decision.needed` events

## Conventions

- Rust 2024 edition
- `thiserror` for library errors, `anyhow` for binary/CLI errors
- Return `Result` types, don't panic in library code
- `axum` for HTTP, `tokio-tungstenite` for WebSocket
- `clap` derive API for CLI
- Tests use `#[tokio::test]` for async, real storage (no mocks)
