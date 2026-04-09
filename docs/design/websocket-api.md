# WebSocket API

Real-time awareness feed. Clients subscribe on connect, receive events as they happen.

## Connection

```
ws://pulse.example.com/ws?repo=<repo-id>
```

## Events

### `workspace.created`

A new workspace was created.

```json
{
  "event": "workspace.created",
  "workspace": {
    "id": "ws-a7f3",
    "intent": "Add JWT auth to API",
    "scope": ["src/auth/*"],
    "author": { "type": "agent", "id": "claude-sonnet-4" }
  }
}
```

### `workspace.committed`

A commit landed in a workspace. Fires immediately.

```json
{
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

### `overlap.detected`

Two workspaces are touching the same files. Informational, non-blocking.

```json
{
  "event": "overlap.detected",
  "file": "src/auth/jwt.rs",
  "workspaces": [
    { "id": "ws-a7f3", "intent": "Add JWT auth to API", "author": { "type": "agent", "id": "claude-sonnet-4" } },
    { "id": "ws-b2c1", "intent": "Add token refresh endpoint", "author": { "type": "agent", "id": "claude-sonnet-4" } }
  ]
}
```

### `decision.needed`

A merge failed due to conflict. Broadcast to all subscribers. Whoever picks it up resolves it.

```json
{
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

### `trunk.updated`

Trunk moved forward.

```json
{
  "event": "trunk.updated",
  "changeset": {
    "id": "<hash>",
    "message": "Add JWT auth to API",
    "author": { "type": "agent", "id": "claude-sonnet-4" },
    "from_workspace": "ws-a7f3"
  }
}
```

### `workspace.merged` / `workspace.abandoned`

Workspace lifecycle events.

### `offline.replay.started` / `offline.replay.conflict`

Fired when a reconnecting client begins draining its offline queue, or when a replayed commit hits a conflict.
