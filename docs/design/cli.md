# CLI

The CLI is a thin REST/WebSocket client baked into the same binary.

## Server

```bash
seven server start                          # hosted mode
seven server start --local                  # localhost mode
seven server transfer --to https://...      # migrate repo to remote
```

## Init

```bash
seven init --remote https://seven.example.com
seven init --local
```

## Workspaces

```bash
seven workspace create --intent "Add JWT auth" --scope "src/auth/*"
seven workspace list
seven workspace status ws-a7f3
```

## Work

```bash
seven commit --message "Implement JWT tokens" --workspace ws-a7f3
seven merge ws-a7f3
```

## Query

```bash
seven log
seven log --author agent:claude-sonnet-4
seven diff HEAD~1 HEAD
seven status
```

## Offline

```bash
seven queue status                          # show buffered commits
seven queue drain                           # force replay (auto on reconnect)
```
