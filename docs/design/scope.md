# Scope

## What's in MVP

- Single binary: server + storage engine + CLI
- Hosted and local deployment modes
- Append-only storage with BLAKE3 + FastCDC + zstd
- Chunk deduplication
- Core primitives: chunk, blob, snapshot, changeset, trunk, workspace
- REST API for all operations
- WebSocket feed for real-time events
- File-level merge with conflict detection
- Overlap detection (advisory)
- Offline write buffer with reconnect replay
- Repo transfer (local -> remote migration)

## Deferred

| Feature | Why deferred | Extension point |
|---------|-------------|-----------------|
| Token/character-level tracking | File-level first | `metadata` field on changesets |
| Intent enforcement | Advisory scope is enough | `scope` field on workspaces |
| Commutative patches | Three-way merge works for now | Replace merge engine |
| Conflict auto-resolution | Flag and fail is honest | `decision.needed` event |
| Authentication / permissions | Trust model TBD | API layer |
| Multi-repo | One repo at a time | Service routing |
| Workspace-to-workspace merge | Everything goes through trunk | Workspace model |
| Failed attempt tracking | Needs design pass | `metadata` field |
| zstd dictionary training | Works fine without it initially | Storage engine config |
| Read cache for offline | Write buffer only at MVP | Client layer |
| Event filtering / subscriptions | All subscribers get all events | WebSocket API |

## Open Questions

1. **Conflict resolution protocol** — when `decision.needed` fires, how does an agent or human "claim" the resolution? Locking? First-write-wins?
2. **Workspace rebasing** — if trunk moves forward while a workspace is active, should the workspace auto-rebase? Or merge against whatever trunk was when the merge is requested?
3. **Chunk size tuning** — FastCDC's min/avg/max chunk sizes affect dedup ratio vs. overhead. Starting point: 2KB/8KB/32KB. Needs benchmarking on real codebases.
4. **Index persistence** — rebuild from log on startup (simple, slow for large repos) or persist index snapshots (faster startup, more complexity)?
5. **Compaction strategy** — how often? Space threshold or time interval? Background thread?
6. **Transfer protocol** — stream the raw log file or replay through the API? Raw is faster, API replay is safer.
7. **Agent commit latency** — agents commit fast and often. Target for the commit->store->notify pipeline? Sub-100ms?
