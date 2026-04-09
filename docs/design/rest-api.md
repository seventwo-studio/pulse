# REST API

All operations. Stateless. Same endpoints for CLI, editors, and agents.

## Repository

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/repo/init` | Create a new repository |
| `GET` | `/repo/status` | Current trunk head, active workspaces |
| `POST` | `/repo/transfer` | Initiate repo transfer to another server |

## Trunk

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/trunk` | Current trunk changeset |
| `GET` | `/trunk/log` | Changeset history (`?author=`, `?since=`) |
| `GET` | `/trunk/snapshot` | Current trunk snapshot (file manifest) |

## Objects

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/objects/:hash` | Retrieve a blob or chunk |
| `POST` | `/objects` | Store a blob, returns hash |
| `POST` | `/objects/batch` | Store multiple blobs (chunking happens server-side) |
| `POST` | `/objects/have` | "Which of these hashes do you already have?" For efficient transfer |

## Workspaces

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/workspaces` | Create workspace (`{ intent, scope, author }`) |
| `GET` | `/workspaces` | List active workspaces |
| `GET` | `/workspaces/:id` | Workspace detail + changeset list |
| `POST` | `/workspaces/:id/commit` | Commit to workspace (instantly synced) |
| `POST` | `/workspaces/:id/merge` | Merge workspace into trunk |
| `DELETE` | `/workspaces/:id` | Abandon workspace |

## Diff & Query

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/diff/:a/:b` | File-level diff between two changesets |
| `GET` | `/files/:path` | File content at trunk head |
| `GET` | `/files/:path?ref=:changeset` | File content at specific changeset |
