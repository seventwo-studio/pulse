# Pulse Design

Pulse is a version control system built for AI-native development. No Git compatibility. No historical baggage. Remote-first, multiplayer-first. Single Rust binary.

Multiple AI agents and humans work in parallel on a shared codebase with real-time awareness of each other's activity. Single main. Ephemeral workspaces. Instant sync.

## Design Documents

| Document | Covers |
|----------|--------|
| [Architecture](./architecture.md) | Binary structure, deployment modes, repo transfer |
| [Storage Engine](./storage-engine.md) | Append-only log, content pipeline, chunk dedup, on-disk layout |
| [Chunking](./chunking.md) | Structural chunking algorithm — boundary detection, splitting, dedup characteristics |
| [Primitives](./primitives.md) | Chunk, blob, snapshot, changeset, main, workspace |
| [Network Model](./network-model.md) | Source of truth, offline buffer, reconnect replay |
| [REST API](./rest-api.md) | All HTTP endpoints |
| [WebSocket API](./websocket-api.md) | Real-time event feed and event types |
| [Merge & Overlap](./merge-and-overlap.md) | Merge strategy, overlap detection |
| [CLI](./cli.md) | Command reference |
| [**Server Spec**](./server-spec.md) | **Full implementation spec: state, endpoints, algorithms, concurrency** |
| [Scope](./scope.md) | What's in MVP, what's deferred, open questions |
