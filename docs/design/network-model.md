# Network Model

## Source of Truth

The server is always the source of truth. Every commit targets the server. There is no local repository, no local history, no push/pull cycle.

```
Agent/CLI ──commit──> Server (source of truth)
                          │
                          ├── stored in append-only log
                          ├── index updated
                          └── WebSocket subscribers notified
```

## Offline Buffer

When the server is unreachable, the client buffers commits locally and replays them in order on reconnect.

```
Online:    commit -> server (immediate)
Offline:   commit -> local queue
Reconnect: queue drains -> replay commits to server in order
```

The local queue is a **write buffer, not a repository**. No local history browsing, no local diffing, no local branching. Just an ordered list of commits waiting to be sent.

## Reconnect Replay

1. Commits replay in the order they were made
2. If a replayed commit conflicts with work that landed on the server while offline, `decision.needed` fires
3. Remaining queued commits pause until the conflict is resolved

This is a deliberate tradeoff. Offline is degraded mode, not full-featured mode. The system is designed for always-connected agents and developers, with the buffer as a safety net.
