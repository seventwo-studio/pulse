# Core Primitives

## Chunk

The fundamental storage unit. A variable-size slice of file content produced by the [structural chunker](./chunking.md).

```
BLAKE3(content) -> chunk hash
chunk hash -> compressed bytes in append-only log
```

Clients never interact with chunks directly. They work with files and blobs. Chunking is internal to the storage engine. See [Chunking](./chunking.md) for how split points are chosen.

## Blob

A file's content, represented as an ordered list of chunk hashes.

```json
{
  "hash": "<BLAKE3 hash of full file content>",
  "chunks": ["<chunk-hash>", "<chunk-hash>"]
}
```

To reconstruct a file: look up each chunk by hash, decompress, concatenate.

## Snapshot

A complete picture of the project at a point in time. Flat path-to-blob map. No directory objects.

```json
{
  "id": "<hash>",
  "files": {
    "src/main.rs": "<blob-hash>",
    "src/lib.rs": "<blob-hash>",
    "README.md": "<blob-hash>"
  }
}
```

## Changeset

A recorded transition between two snapshots. The fundamental unit of history.

```json
{
  "id": "<hash>",
  "parent": "<parent-changeset-id | null>",
  "snapshot": "<snapshot-id>",
  "timestamp": "2026-04-08T14:30:00Z",
  "author": {
    "type": "agent | human",
    "id": "claude-sonnet-4",
    "session": "ws-a7f3"
  },
  "message": "Refactored auth module to use JWT",
  "files_changed": ["src/auth/jwt.rs", "src/auth/mod.rs"],
  "metadata": {}
}
```

- **`author.type` and `author.id` are mandatory.** Every change knows if it came from a human or AI, and which model/agent.
- **`files_changed` is mandatory.** Powers the awareness layer without diffing snapshots on every commit.

## Trunk

Single linear history. The source of truth. One ref, one line.

```
trunk -> changeset-id
```

## Release

A named label on a trunk changeset with a lifecycle state. Releases always point forward — no backports, no inserting history.

```json
{
  "id": "rel-x9k2",
  "name": "v2.4.0",
  "changeset": "<changeset-hash>",
  "snapshot": "<snapshot-id>",
  "status": "ready | testing | live | abandoned",
  "author": {
    "type": "human",
    "id": "luca"
  },
  "created": "2026-04-09T10:00:00Z",
  "metadata": {}
}
```

Key properties:

- **Points to a trunk changeset.** A release is a label, not a branch. It refers to an immutable point in trunk history.
- **Always forward.** A new release must point to the same or a later changeset than the previous release. No rewinding.
- **Lifecycle states:**
  - `ready` — changeset selected, release created, not yet validated
  - `testing` — being validated (CI, staging, QA)
  - `live` — deployed to production
  - `abandoned` — release was cancelled before going live
- **Hotfixes go forward.** Fix lands on trunk, cut a new release from a later changeset. No patching old releases.
- **`metadata` is open.** CI results, deploy URLs, rollback-from references — whatever the platform needs.

## Workspace

An ephemeral, remote-tracked, isolated context for making changes.

```json
{
  "id": "ws-a7f3",
  "base": "<changeset-id>",
  "intent": "Add JWT authentication to the API auth module",
  "scope": ["src/auth/*"],
  "author": {
    "type": "agent",
    "id": "claude-sonnet-4"
  },
  "created": "2026-04-08T14:30:00Z",
  "status": "active | merged | abandoned",
  "changesets": ["<changeset-id>"]
}
```

Key properties:

- **Always remote.** Created on the server, immediately visible to all subscribers.
- **Intent declared upfront.** The agent says *why* it needs this workspace.
- **Scope declared upfront.** Which files/dirs the agent expects to touch. Advisory, not enforced — but powers overlap detection.
- **Ephemeral.** Fork from trunk, work, merge back, gone.
