# REST API

All operations. Stateless. Same endpoints for CLI, editors, and agents.

## Repository

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/repo/init` | Create a new repository |
| `GET` | `/repo/status` | Current main head, active workspaces |
| `POST` | `/repo/transfer` | Initiate repo transfer to another server |

## Main

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/main` | Current main changeset |
| `GET` | `/main/log` | Changeset history (`?author=`, `?since=`) |
| `GET` | `/main/snapshot` | Current main snapshot (file manifest) |

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
| `POST` | `/workspaces/:id/merge` | Merge workspace into main |
| `DELETE` | `/workspaces/:id` | Abandon workspace |

## Releases

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/releases` | Create release (`{ name, changeset, author }`) — status starts at `ready` |
| `GET` | `/releases` | List releases (`?status=`, `?since=`) |
| `GET` | `/releases/:id` | Release detail |
| `GET` | `/releases/latest` | Most recent `live` release |
| `PATCH` | `/releases/:id` | Update status (`ready → testing → live`, or `→ abandoned`) |

Constraints enforced server-side:
- `changeset` must be on main and at or ahead of the previous release's changeset
- Status transitions: `ready → testing → live`, `ready → abandoned`, `testing → abandoned`. No backwards moves.

## Diff & Query

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/diff/:a/:b` | File-level diff between two changesets |
| `GET` | `/files/:path` | File content at main head |
| `GET` | `/files/:path?ref=:changeset` | File content at specific changeset |
