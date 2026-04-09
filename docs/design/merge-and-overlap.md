# Merge & Overlap Detection

## Merge Strategy

File-level granularity. Same file touched by both sides = conflict.

1. Find common ancestor (workspace's `base` changeset)
2. Diff workspace snapshot vs. ancestor — files changed in workspace
3. Diff trunk snapshot vs. ancestor — files changed in trunk since workspace was created
4. **No overlap** — auto-merge, apply both sets of changes
5. **Overlap** — fail, emit `decision.needed` event with both versions of conflicting files

No line-level merge. No auto-resolution. Flag and fail is honest.

## Overlap Detection

Triggered when:

- A workspace is **created** with scope overlapping an existing active workspace's scope
- A **commit** lands in a workspace that touches files in another active workspace's scope or recent commits

Detection is **advisory, non-blocking**. The system tells you about it. You decide what to do.

### Scope Matching

Glob patterns with simple prefix matching at MVP:

- `src/auth/*` overlaps `src/auth/jwt.rs` — yes
- `src/auth/*` overlaps `src/api/routes.rs` — no

## Agent Workflow Example

```
Agent A: create workspace for JWT auth in src/auth/
  -> POST /workspaces { intent: "Add JWT auth", scope: ["src/auth/*"] }
  -> ws-a7f3 created
  -> all subscribers notified: workspace.created

Agent B: create workspace for token refresh in src/auth/
  -> POST /workspaces { intent: "Add token refresh", scope: ["src/auth/*"] }
  -> ws-b2c1 created
  -> overlap.detected: both scoped to src/auth/*
  -> each agent sees what the other is working on

Agent A commits and merges:
  -> no conflict with trunk, merged
  -> trunk.updated + workspace.merged

Agent B commits and merges:
  -> conflict: src/auth/mod.rs changed by both
  -> decision.needed fired
  -> human or designated agent resolves
```
