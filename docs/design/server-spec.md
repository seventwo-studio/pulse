# Server Specification

This document specifies the behavior of the Seven server precisely enough that anyone can build a conformant implementation. It covers state management, every endpoint's contract, the WebSocket protocol, storage operations, concurrency rules, and the merge algorithm.

The design docs describe *what* Seven is. This spec describes *how the server must behave*.

---

## Table of Contents

- [State Model](#state-model)
- [Server Lifecycle](#server-lifecycle)
- [Storage Operations](#storage-operations)
- [REST Endpoints](#rest-endpoints)
- [WebSocket Protocol](#websocket-protocol)
- [Merge Algorithm](#merge-algorithm)
- [Overlap Detection Algorithm](#overlap-detection-algorithm)
- [Concurrency Model](#concurrency-model)
- [Repo Transfer Protocol](#repo-transfer-protocol)
- [Error Model](#error-model)

---

## State Model

The server holds two categories of state: **durable** (on disk) and **volatile** (in memory, rebuilt on startup).

### Durable State

All durable state lives under `.seven/`:

| File | Contents | Format |
|------|----------|--------|
| `data/chunks.log` | Compressed chunk bytes | Append-only binary log |
| `data/chunks.index` | Hash-to-offset index snapshot | Binary, optional (rebuilt if missing) |
| `meta/changesets.log` | Changeset records | Append-only, length-prefixed JSON |
| `meta/snapshots.log` | Snapshot manifests | Append-only, length-prefixed JSON |
| `meta/workspaces.log` | Workspace lifecycle events | Append-only, length-prefixed JSON |
| `meta/trunk` | Current trunk changeset ID | Single line, UTF-8 |
| `config.toml` | Server config, repo metadata | TOML |

Every log file uses the same framing: `[4-byte little-endian length][payload][4-byte BLAKE3 checksum of payload]`. The checksum enables truncation detection on startup.

### Volatile State

Rebuilt from the logs on startup:

| Structure | Source | Purpose |
|-----------|--------|---------|
| Chunk index | `chunks.log` + `chunks.index` | `hash -> (offset, length)` for chunk lookups |
| Changeset index | `changesets.log` | `id -> Changeset` for history traversal |
| Snapshot index | `snapshots.log` | `id -> Snapshot` for file lookups |
| Workspace table | `workspaces.log` | Active workspaces, their state and scopes |
| Trunk pointer | `meta/trunk` | Current head changeset ID |

If `chunks.index` exists, load it and verify its generation marker matches `chunks.log`. If stale or missing, rebuild by scanning `chunks.log` sequentially.

---

## Server Lifecycle

### Startup

1. Open or create `.seven/` directory structure
2. Scan each log file. For every log:
   a. Read entries sequentially, validating checksums
   b. If the final entry has an invalid or incomplete checksum, truncate it (crash recovery)
   c. Build the corresponding in-memory index
3. Load `meta/trunk` (empty string if repo is uninitialized)
4. Load `config.toml`
5. Start the HTTP listener (REST + WebSocket upgrade)
6. Server is ready. Log the listening address and trunk head

### Shutdown

1. Stop accepting new connections
2. Drain in-flight requests (bounded timeout, e.g. 10s)
3. Close all WebSocket connections with code `1001` (Going Away)
4. Persist `data/chunks.index` snapshot
5. Fsync all open log files
6. Exit

### Crash Recovery

On startup, if the last entry in any log has a bad or partial checksum:
- Truncate the file to the end of the last valid entry
- Log a warning with the file name and bytes discarded
- Continue startup normally

This is safe because all log entries are self-contained. An incomplete trailing write cannot corrupt prior entries.

---

## Storage Operations

### Storing Chunks

```
store_chunks(file_content: bytes) -> Blob
```

1. Run FastCDC over `file_content` with parameters: min 2KB, avg 8KB, max 32KB
2. For each chunk produced:
   a. Compute `hash = BLAKE3(chunk_bytes)`
   b. Check if `hash` exists in chunk index. If yes, skip to next chunk
   c. Compress `chunk_bytes` with zstd (default level 3)
   d. Append to `chunks.log`: `[length][compressed_bytes][checksum]`
   e. Record in chunk index: `hash -> (offset_of_this_entry, compressed_length)`
3. Compute `blob_hash = BLAKE3(file_content)`
4. Return `Blob { hash: blob_hash, chunks: [chunk_hashes_in_order] }`

### Retrieving a File

```
retrieve_file(blob: Blob) -> bytes
```

1. For each `chunk_hash` in `blob.chunks`:
   a. Look up `(offset, length)` in chunk index
   b. Seek to `offset` in `chunks.log`, read `length` bytes
   c. Decompress with zstd
2. Concatenate all decompressed chunks in order
3. Verify `BLAKE3(result) == blob.hash` (optional integrity check)
4. Return result

### Building a Snapshot

```
build_snapshot(files: Map<path, blob_hash>) -> Snapshot
```

1. Sort files by path (lexicographic, UTF-8)
2. Serialize the sorted map as canonical JSON
3. Compute `id = BLAKE3(serialized)`
4. Append record to `snapshots.log`
5. Index in memory
6. Return `Snapshot { id, files }`

### Creating a Changeset

```
create_changeset(parent, snapshot_id, author, message, files_changed) -> Changeset
```

1. Compute `id = BLAKE3(parent + snapshot_id + timestamp + author + message)`
2. Construct changeset record
3. Append to `changesets.log`
4. Index in memory
5. Return changeset

---

## REST Endpoints

Every endpoint returns JSON. All request bodies are JSON. Content-Type is always `application/json`.

Errors use the format in the [Error Model](#error-model) section.

### `POST /repo/init`

Initialize a new repository.

**Precondition**: Trunk pointer is empty (repo not yet initialized).

**Request body**: None.

**Behavior**:
1. Create an empty snapshot (no files)
2. Create a root changeset with `parent: null`, the empty snapshot, and `author: { type: "system", id: "seven" }`
3. Write the root changeset ID to `meta/trunk`

**Response** `201`:
```json
{
  "changeset": "<root-changeset-id>",
  "message": "Repository initialized"
}
```

**Error** `409`: Repository already initialized.

---

### `GET /repo/status`

**Response** `200`:
```json
{
  "trunk": "<changeset-id>",
  "active_workspaces": 3
}
```

Returns `trunk: null` if repo is uninitialized.

---

### `POST /repo/transfer`

Initiate a repo transfer to another server. See [Repo Transfer Protocol](#repo-transfer-protocol).

**Request body**:
```json
{
  "target": "https://seven.example.com"
}
```

**Behavior**: Starts the transfer process in the background. Returns immediately.

**Response** `202`:
```json
{
  "transfer_id": "<uuid>",
  "status": "started"
}
```

---

### `GET /trunk`

**Response** `200`:
```json
{
  "id": "<changeset-id>",
  "parent": "<parent-id>",
  "snapshot": "<snapshot-id>",
  "timestamp": "2026-04-08T14:30:00Z",
  "author": { "type": "agent", "id": "claude-sonnet-4", "session": "ws-a7f3" },
  "message": "Refactored auth module",
  "files_changed": ["src/auth/jwt.rs"]
}
```

**Error** `404`: Repo not initialized.

---

### `GET /trunk/log`

**Query parameters**:
- `author` (optional): filter by `author.id` (exact match)
- `since` (optional): ISO 8601 timestamp, return only changesets after this time
- `limit` (optional): max number of results, default 50, max 1000

**Response** `200`:
```json
{
  "changesets": [
    { "id": "...", "parent": "...", "timestamp": "...", "author": {...}, "message": "...", "files_changed": [...] }
  ]
}
```

Ordered newest-first. Walk the parent chain from trunk head backwards.

---

### `GET /trunk/snapshot`

**Response** `200`:
```json
{
  "id": "<snapshot-id>",
  "files": {
    "src/main.rs": "<blob-hash>",
    "src/lib.rs": "<blob-hash>"
  }
}
```

---

### `GET /objects/:hash`

Retrieve a blob (chunk list) or raw chunk data.

**Query parameters**:
- `type` (optional): `blob` or `chunk`. If omitted, server tries blob first, then chunk.

**Response** `200` (blob):
```json
{
  "type": "blob",
  "hash": "<hash>",
  "chunks": ["<chunk-hash>", "..."]
}
```

**Response** `200` (chunk): Raw binary, `Content-Type: application/octet-stream`. Decompressed.

**Error** `404`: Hash not found.

---

### `POST /objects`

Store file content as a blob.

**Request body**: Raw file bytes, `Content-Type: application/octet-stream`.

**Behavior**:
1. Run `store_chunks` on the body
2. Persist the blob record

**Response** `201`:
```json
{
  "hash": "<blob-hash>",
  "chunks": ["<chunk-hash>", "..."],
  "new_chunks": 3,
  "reused_chunks": 12
}
```

---

### `POST /objects/batch`

Store multiple files at once.

**Request body**:
```json
{
  "files": {
    "src/main.rs": "<base64-encoded content>",
    "src/lib.rs": "<base64-encoded content>"
  }
}
```

**Response** `201`:
```json
{
  "blobs": {
    "src/main.rs": { "hash": "<blob-hash>", "chunks": [...] },
    "src/lib.rs": { "hash": "<blob-hash>", "chunks": [...] }
  },
  "stats": { "new_chunks": 5, "reused_chunks": 28 }
}
```

---

### `POST /objects/have`

Check which hashes the server already has. For efficient transfer — the client sends hashes it has, the server reports which ones are already stored.

**Request body**:
```json
{
  "hashes": ["<hash>", "<hash>", "..."]
}
```

**Response** `200`:
```json
{
  "have": ["<hash>", "<hash>"],
  "missing": ["<hash>"]
}
```

---

### `POST /workspaces`

Create a new workspace.

**Request body**:
```json
{
  "intent": "Add JWT authentication to the API",
  "scope": ["src/auth/*"],
  "author": { "type": "agent", "id": "claude-sonnet-4" }
}
```

**Behavior**:
1. Generate workspace ID: `ws-` + 4 random hex chars
2. Set `base` to current trunk head
3. Set `status` to `active`
4. Append creation event to `workspaces.log`
5. Update workspace table in memory
6. Run overlap detection against all other active workspaces
7. Broadcast `workspace.created` to all WebSocket subscribers
8. If overlap detected, broadcast `overlap.detected` for each overlapping pair

**Response** `201`:
```json
{
  "id": "ws-a7f3",
  "base": "<changeset-id>",
  "intent": "Add JWT authentication to the API",
  "scope": ["src/auth/*"],
  "author": { "type": "agent", "id": "claude-sonnet-4" },
  "created": "2026-04-08T14:30:00Z",
  "status": "active",
  "overlaps": [
    { "workspace_id": "ws-b2c1", "intent": "Add token refresh", "matched_scopes": ["src/auth/*"] }
  ]
}
```

The `overlaps` field is informational — the workspace is created regardless.

---

### `GET /workspaces`

**Response** `200`:
```json
{
  "workspaces": [
    {
      "id": "ws-a7f3",
      "intent": "...",
      "scope": ["..."],
      "author": {...},
      "created": "...",
      "status": "active",
      "changeset_count": 3
    }
  ]
}
```

Only returns workspaces with `status: active` by default. Pass `?all=true` for all statuses.

---

### `GET /workspaces/:id`

**Response** `200`:
```json
{
  "id": "ws-a7f3",
  "base": "<changeset-id>",
  "intent": "...",
  "scope": ["..."],
  "author": {...},
  "created": "...",
  "status": "active",
  "changesets": [
    { "id": "...", "message": "...", "timestamp": "...", "files_changed": [...] }
  ]
}
```

**Error** `404`: Workspace not found.

---

### `POST /workspaces/:id/commit`

Commit changes to a workspace.

**Request body**:
```json
{
  "message": "Implement JWT token generation",
  "files": {
    "src/auth/jwt.rs": "<base64-encoded content>",
    "src/auth/mod.rs": "<base64-encoded content>"
  },
  "deleted": ["src/auth/old_auth.rs"]
}
```

**Behavior**:
1. Verify workspace exists and `status == active`
2. Store all file contents via `store_chunks` (→ blob hashes)
3. Build new snapshot:
   - Start from the workspace's latest snapshot (or base changeset's snapshot if first commit)
   - Apply file additions/modifications from `files`
   - Remove paths listed in `deleted`
4. Create changeset with `parent` = workspace's latest changeset (or `base` if first commit)
5. Append changeset to workspace's changeset list in `workspaces.log`
6. Broadcast `workspace.committed` to all WebSocket subscribers
7. Run overlap detection: compare `files_changed` against all other active workspaces' scopes and recent commits. If overlap, broadcast `overlap.detected`

**Response** `201`:
```json
{
  "changeset": {
    "id": "<hash>",
    "parent": "<parent-hash>",
    "snapshot": "<snapshot-id>",
    "timestamp": "2026-04-08T14:32:00Z",
    "message": "Implement JWT token generation",
    "files_changed": ["src/auth/jwt.rs", "src/auth/mod.rs"],
    "files_deleted": ["src/auth/old_auth.rs"]
  },
  "stats": { "new_chunks": 4, "reused_chunks": 18 }
}
```

**Error** `404`: Workspace not found.
**Error** `409`: Workspace is not active (already merged or abandoned).

---

### `POST /workspaces/:id/merge`

Merge a workspace into trunk.

**Request body**: None.

**Behavior**: Execute the [Merge Algorithm](#merge-algorithm). On success:

1. Update `meta/trunk` to the new changeset ID
2. Set workspace `status` to `merged` in `workspaces.log`
3. Broadcast `trunk.updated` to all WebSocket subscribers
4. Broadcast `workspace.merged` to all WebSocket subscribers

**Response** `200` (success):
```json
{
  "merged": true,
  "changeset": {
    "id": "<new-trunk-head>",
    "parent": "<previous-trunk-head>",
    "snapshot": "<merged-snapshot-id>",
    "message": "Merge ws-a7f3: Add JWT authentication to the API",
    "files_changed": ["src/auth/jwt.rs", "src/auth/mod.rs"]
  }
}
```

**Response** `409` (conflict):
```json
{
  "merged": false,
  "conflicts": [
    {
      "file": "src/auth/mod.rs",
      "trunk_changeset": "<hash>",
      "workspace_changeset": "<hash>"
    }
  ]
}
```

On conflict, also broadcast `decision.needed` via WebSocket.

**Error** `404`: Workspace not found.
**Error** `409`: Workspace is not active.

---

### `DELETE /workspaces/:id`

Abandon a workspace.

**Behavior**:
1. Set workspace `status` to `abandoned` in `workspaces.log`
2. Broadcast `workspace.abandoned` to all WebSocket subscribers

Chunks and changesets created by this workspace are **not** deleted. Compaction reclaims them later if unreferenced.

**Response** `200`:
```json
{
  "id": "ws-a7f3",
  "status": "abandoned"
}
```

---

### `GET /diff/:a/:b`

Diff two changesets at the file level.

**Behavior**:
1. Look up snapshot for changeset `a` and changeset `b`
2. Compare file maps: added, removed, modified (different blob hash)

**Response** `200`:
```json
{
  "base": "<changeset-a>",
  "target": "<changeset-b>",
  "added": ["src/new_file.rs"],
  "removed": ["src/old_file.rs"],
  "modified": ["src/changed_file.rs"]
}
```

**Error** `404`: One or both changesets not found.

---

### `GET /files/:path`

Get file content at trunk head. The `:path` is the full file path, URL-encoded (e.g., `/files/src%2Fauth%2Fjwt.rs`).

**Query parameters**:
- `ref` (optional): changeset ID. Defaults to trunk head.

**Behavior**:
1. Look up snapshot for the target changeset
2. Find the blob hash for the requested path
3. Retrieve and reconstruct the file from chunks

**Response** `200`: Raw file bytes, `Content-Type: application/octet-stream`.

**Error** `404`: File not found in snapshot, or changeset not found.

---

## WebSocket Protocol

### Connection

Clients connect to `ws://<host>/ws`. The server upgrades the HTTP connection to WebSocket.

On connect:
1. Server registers the connection as a subscriber
2. Server sends a `connected` frame:
```json
{
  "event": "connected",
  "trunk": "<current-trunk-head>",
  "active_workspaces": 3
}
```

### Message Direction

- **Server -> Client**: Event broadcasts. The server pushes all events to all connected subscribers.
- **Client -> Server**: Not used at MVP. The WebSocket is a one-way event feed. All mutations go through the REST API.

### Event Delivery

Events are delivered as JSON text frames, one event per frame. No batching. No acknowledgment protocol — delivery is best-effort over the WebSocket.

If the connection drops, the client misses events. On reconnect, the client should query the REST API to catch up (e.g., `GET /trunk`, `GET /workspaces`).

### Event Reference

Every event has an `event` field (string) and a monotonically increasing `seq` field (integer, scoped to the connection) for client-side ordering and gap detection.

```json
{ "seq": 42, "event": "...", ... }
```

#### `workspace.created`
Fired when a workspace is created via `POST /workspaces`.
```json
{
  "seq": 1,
  "event": "workspace.created",
  "workspace": {
    "id": "ws-a7f3",
    "intent": "Add JWT auth to API",
    "scope": ["src/auth/*"],
    "author": { "type": "agent", "id": "claude-sonnet-4" }
  }
}
```

#### `workspace.committed`
Fired when a commit lands via `POST /workspaces/:id/commit`.
```json
{
  "seq": 2,
  "event": "workspace.committed",
  "workspace_id": "ws-a7f3",
  "changeset": {
    "id": "<hash>",
    "message": "Implement JWT token generation",
    "files_changed": ["src/auth/jwt.rs"],
    "author": { "type": "agent", "id": "claude-sonnet-4" }
  }
}
```

#### `overlap.detected`
Fired when scope or file overlap is found between active workspaces.
```json
{
  "seq": 3,
  "event": "overlap.detected",
  "file": "src/auth/jwt.rs",
  "workspaces": [
    { "id": "ws-a7f3", "intent": "Add JWT auth", "author": { "type": "agent", "id": "claude-sonnet-4" } },
    { "id": "ws-b2c1", "intent": "Add token refresh", "author": { "type": "agent", "id": "claude-sonnet-4" } }
  ]
}
```

#### `trunk.updated`
Fired when a merge succeeds and trunk advances.
```json
{
  "seq": 4,
  "event": "trunk.updated",
  "changeset": {
    "id": "<hash>",
    "message": "Add JWT auth to API",
    "author": { "type": "agent", "id": "claude-sonnet-4" },
    "from_workspace": "ws-a7f3"
  }
}
```

#### `decision.needed`
Fired when a merge fails due to conflict.
```json
{
  "seq": 5,
  "event": "decision.needed",
  "type": "merge_conflict",
  "workspace_id": "ws-b2c1",
  "conflicts": [
    {
      "file": "src/auth/jwt.rs",
      "trunk_changeset": "<hash>",
      "workspace_changeset": "<hash>"
    }
  ]
}
```

#### `workspace.merged`
Fired after a successful merge. Workspace is no longer active.
```json
{
  "seq": 6,
  "event": "workspace.merged",
  "workspace_id": "ws-a7f3",
  "trunk_changeset": "<hash>"
}
```

#### `workspace.abandoned`
Fired when a workspace is deleted.
```json
{
  "seq": 7,
  "event": "workspace.abandoned",
  "workspace_id": "ws-a7f3"
}
```

#### `offline.replay.started`
Fired when a client begins draining its offline queue through the REST API. Informational — lets other subscribers know replayed commits are incoming.
```json
{
  "seq": 8,
  "event": "offline.replay.started",
  "author": { "type": "human", "id": "luca" },
  "queued_commits": 4
}
```

#### `offline.replay.conflict`
Fired when a replayed offline commit conflicts with current trunk.
```json
{
  "seq": 9,
  "event": "offline.replay.conflict",
  "author": { "type": "human", "id": "luca" },
  "workspace_id": "ws-c3d2",
  "conflict_file": "src/main.rs"
}
```

### Connection Lifecycle

- Server sends `ping` frames every 30 seconds
- If a client doesn't respond with `pong` within 10 seconds, the server closes the connection
- On abnormal close, the server removes the subscriber immediately

---

## Merge Algorithm

When `POST /workspaces/:id/merge` is called:

### Inputs
- `workspace`: the workspace being merged
- `base`: the workspace's `base` changeset (trunk head when workspace was created)
- `workspace_head`: the last changeset in the workspace's changeset list
- `trunk_head`: current trunk head

### Fast-Forward Case

If `trunk_head == base` (trunk hasn't moved since workspace was created):

1. Build a merge changeset:
   - `parent`: `trunk_head`
   - `snapshot`: `workspace_head`'s snapshot
   - `files_changed`: union of all `files_changed` across workspace's changesets
   - `message`: `"Merge {workspace.id}: {workspace.intent}"`
   - `author`: workspace's author
2. Update trunk to the merge changeset
3. Return success

### Three-Way Merge Case

If `trunk_head != base` (trunk advanced while workspace was active):

1. Resolve the three snapshots:
   - `ancestor_snapshot` = snapshot of `base`
   - `trunk_snapshot` = snapshot of `trunk_head`
   - `workspace_snapshot` = snapshot of `workspace_head`

2. Compute changed file sets:
   - `trunk_changed` = files where `trunk_snapshot[path] != ancestor_snapshot[path]` (including additions and deletions)
   - `workspace_changed` = files where `workspace_snapshot[path] != ancestor_snapshot[path]`

3. Check for conflicts:
   - `conflicting_files` = `trunk_changed ∩ workspace_changed`
   - If `conflicting_files` is non-empty → **conflict**, return failure with the list

4. Build merged snapshot:
   - Start from `ancestor_snapshot`
   - Apply all changes from `trunk_snapshot` (the trunk side wins for trunk-only changes)
   - Apply all changes from `workspace_snapshot` (the workspace side wins for workspace-only changes)
   - Result: a snapshot with both sets of non-overlapping changes

5. Build merge changeset:
   - `parent`: `trunk_head`
   - `snapshot`: the merged snapshot
   - `files_changed`: `trunk_changed ∪ workspace_changed`
   - `message`: `"Merge {workspace.id}: {workspace.intent}"`
   - `author`: workspace's author

6. Update trunk to the merge changeset
7. Return success

### Conflict Handling

On conflict:
1. Do **not** update trunk
2. Do **not** change workspace status (stays `active`)
3. Return `409` with the conflict list
4. Broadcast `decision.needed` via WebSocket
5. The workspace remains active — the author (or another agent/human) can make additional commits to the workspace to resolve the conflict, then retry the merge

---

## Overlap Detection Algorithm

### When It Runs

1. **On workspace creation**: compare the new workspace's `scope` against every active workspace's `scope`
2. **On commit**: compare the commit's `files_changed` against every other active workspace's `scope` and `files_changed` from their recent commits

### Scope Matching

Scopes are glob patterns. At MVP, matching uses prefix rules:

- A scope pattern `dir/*` matches any path starting with `dir/`
- A scope pattern `dir/file.rs` matches exactly `dir/file.rs`
- Two scopes overlap if either could match a path the other could match

Implementation: convert globs to prefix strings and check for prefix containment. `src/auth/*` and `src/auth/jwt.rs` overlap because `src/auth/` is a prefix of `src/auth/jwt.rs`.

### On Detection

Overlap detection is **advisory only**. It never blocks an operation. When overlap is detected:

1. Build the `overlap.detected` event with the overlapping file/scope and both workspaces
2. Broadcast to all WebSocket subscribers
3. Include in the REST response if triggered by workspace creation

---

## Concurrency Model

### Trunk Lock

The merge operation takes an exclusive lock on trunk. Only one merge can execute at a time. This lock is held for the duration of the merge algorithm (snapshot diffing + changeset creation + trunk pointer update).

All other operations (commits to workspaces, reads, object storage) are lock-free with respect to trunk.

### Workspace Isolation

Commits to different workspaces are fully independent and can execute concurrently. Commits to the **same** workspace must be serialized — the server processes them in arrival order. A per-workspace lock ensures this.

### Log Writes

Each log file has its own write lock. Multiple log files can be written concurrently (e.g., storing chunks while writing a changeset record). Within a single log file, writes are serialized.

### WebSocket Broadcasts

Events are dispatched to subscribers asynchronously after the triggering operation completes. A slow subscriber does not block the operation or other subscribers. If a subscriber's send buffer is full, the event is dropped for that subscriber.

---

## Repo Transfer Protocol

When `POST /repo/transfer` is called with a target URL:

### Phase 1: Snapshot

1. Pause new merges (hold trunk lock)
2. Record current trunk head as the transfer point
3. Resume merges

### Phase 2: Stream

4. Stream `data/chunks.log` to the target server via `POST /repo/transfer/receive-chunks` (chunked HTTP transfer)
5. Stream all meta logs (`changesets.log`, `snapshots.log`, `workspaces.log`) to the target
6. Send trunk pointer

### Phase 3: Catch-up

7. Stream any new log entries appended since step 2 started (the delta)
8. Repeat until delta is empty or below a threshold

### Phase 4: Cutover

9. Hold trunk lock on source
10. Send final delta
11. Target server confirms it has rebuilt its indices and is ready
12. Source server responds to all new requests with `301 Moved Permanently` pointing to the target
13. Release trunk lock. Source is now a redirect-only stub

The target server must implement a `POST /repo/transfer/receive-chunks` and `POST /repo/transfer/receive-meta` endpoint pair that accepts streamed log data and rebuilds indices.

---

## Error Model

All errors use a consistent JSON format:

```json
{
  "error": {
    "code": "workspace_not_found",
    "message": "Workspace ws-a7f3 does not exist",
    "status": 404
  }
}
```

### Error Codes

| Code | HTTP Status | Meaning |
|------|-------------|---------|
| `repo_not_initialized` | 404 | Repo has not been initialized yet |
| `repo_already_initialized` | 409 | `POST /repo/init` called on existing repo |
| `workspace_not_found` | 404 | Workspace ID doesn't exist |
| `workspace_not_active` | 409 | Operation requires active workspace but it's merged/abandoned |
| `changeset_not_found` | 404 | Changeset hash doesn't exist |
| `object_not_found` | 404 | Blob or chunk hash doesn't exist |
| `file_not_found` | 404 | Path doesn't exist in the target snapshot |
| `merge_conflict` | 409 | Merge failed due to conflicting files |
| `transfer_in_progress` | 409 | A repo transfer is already running |
| `transfer_failed` | 502 | Target server unreachable or rejected the transfer |
| `internal_error` | 500 | Unexpected server error |
