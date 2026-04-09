# CLI

The CLI is a thin REST/WebSocket client baked into the same binary.

## Server

```bash
pulse server start                          # hosted mode
pulse server start --local                  # localhost mode
pulse server transfer --to https://...      # migrate repo to remote
```

## Init

```bash
pulse init --remote https://pulse.example.com
pulse init --local
```

## Workspaces

```bash
pulse workspace create --intent "Add JWT auth" --scope "src/auth/*"
pulse workspace list
pulse workspace status ws-a7f3
```

## Work

```bash
pulse commit --message "Implement JWT tokens" --workspace ws-a7f3
pulse merge ws-a7f3
```

## Query

```bash
pulse log
pulse log --author agent:claude-sonnet-4
pulse diff HEAD~1 HEAD
pulse status
```

## Offline

```bash
pulse queue status                          # show buffered commits
pulse queue drain                           # force replay (auto on reconnect)
```
