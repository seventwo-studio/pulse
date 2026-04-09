# Architecture

## Single Binary

Everything compiles into one Rust binary: server (REST + WebSocket), storage engine, and CLI client.

```
┌──────────────────────────────────────────────────┐
│                  pulse binary                    │
│                                                  │
│  ┌──────────┐  ┌─────────────┐  ┌────────────┐  │
│  │ REST API │  │ WebSocket   │  │ CLI Client │  │
│  │          │  │ Feed        │  │            │  │
│  └─────┬────┘  └──────┬──────┘  └─────┬──────┘  │
│        │              │               │          │
│  ┌─────┴──────────────┴───────────────┘          │
│  │                                               │
│  │              Core Engine                      │
│  │  ┌─────────────────────────────────────────┐  │
│  │  │         Custom Storage Engine           │  │
│  │  │                                         │  │
│  │  │  BLAKE3 hashing                         │  │
│  │  │  Structural chunking (FastCDC fallback) │  │
│  │  │  zstd compression                       │  │
│  │  │  Append-only log + in-memory index      │  │
│  │  └─────────────────────────────────────────┘  │
│  │                                               │
│  │  workspaces · trunk · awareness layer         │
│  └───────────────────────────────────────────────┘
└──────────────────────────────────────────────────┘
       ▲              ▲              ▲
       │              │              │
  ┌────┴───┐    ┌─────┴────┐   ┌────┴────┐
  │  CLI   │    │  Editor  │   │  Agent  │
  │(human) │    │  Plugin  │   │  (AI)   │
  └────────┘    └──────────┘   └─────────┘
```

All clients use the same protocol. Attribution distinguishes who did what.

## Deployment Modes

**Hosted service** (primary) — Pulse runs as a service. Agents and humans connect over the network.

**Local mode** (secondary) — Same binary, same API, runs on localhost. Useful for solo dev and early prototyping before migrating to hosted.

The client doesn't know the difference. Point it at a URL, it works.

```bash
pulse server start              # run as service
pulse server start --local      # run on localhost
pulse workspace create ...      # CLI talks to server either way
```

## Repo Transfer

A local server can transfer its entire repository to a remote server. This is a **move**, not ongoing sync:

1. Dump the full append-only log + all chunks
2. Replay on the new server
3. Local server stops being the source of truth
4. All clients repoint to the new server

One-time migration. Not bidirectional sync.
