# Storage Engine

Custom embedded storage engine in Rust. No external database dependency.

## Append-Only Log

All data is immutable. Blobs, chunks, changesets, snapshots — once written, never modified.

### Write Path

1. Append data to the end of the data file
2. Update in-memory index: `hash -> (offset, length)`
3. Index is rebuilt from the log on startup (or from a persisted snapshot for fast restart)

### Read Path

1. Look up hash in the in-memory index
2. Seek to offset in data file, read `length` bytes
3. Decompress (zstd), return

### Crash Safety

If power dies mid-write, only the latest incomplete entry is lost. Everything before it is intact — append-only guarantees this. On restart, detect and truncate any incomplete trailing entry.

### Compaction

Periodically copy live data to a new file, discard the old one. Reclaims space from abandoned workspaces and unreferenced chunks. Runs in the background, doesn't block reads or writes.

## Content Pipeline

Every file goes through this pipeline on commit:

```
file content
     │
     ▼
  FastCDC ── content-defined chunking, variable-size (~4KB–16KB)
     │
     ▼
  BLAKE3 ─── hash each chunk, content-addressable
     │
     ▼
   zstd ──── compress each chunk
     │
     ▼
  append ─── write to log, update index: hash -> (offset, len)
```

**FastCDC** splits files at content-determined boundaries. If 3 lines are inserted in the middle of a file, only the chunks around those lines change. The rest are already stored. This gives massive dedup for the edit pattern agents produce — lots of small, frequent changes to code files.

**BLAKE3** is 3–4x faster than SHA-256, parallelizable, with a clean license (CC0 / Apache 2.0).

**zstd** gives strong compression ratios with fast decompression. Dictionary mode can be trained on the repo's codebase for even better ratios on similar code.

## Chunk Deduplication

Before writing a chunk, check if its hash exists in the index. If yes, skip the write. This means:

- Two agents committing similar files store overlapping chunks once
- File renames are free (same content, same chunks, same hashes)
- Small edits to large files only store the changed chunks

## On-Disk Layout

```
.seven/
  data/
    chunks.log            # append-only log of compressed chunks
    chunks.index          # persisted index snapshot (rebuilt on startup if missing)
  meta/
    changesets.log        # append-only log of changeset records
    snapshots.log         # append-only log of snapshot manifests
    workspaces.log        # append-only log of workspace lifecycle events
    trunk                 # current trunk pointer (single changeset ID)
  config.toml             # server config, repo metadata
```

## Dependencies

| Component | License | Notes |
|-----------|---------|-------|
| BLAKE3 | CC0 / Apache 2.0 | Dual-licensed |
| FastCDC | MIT | Algorithm from paper, Rust crate MIT |
| zstd | BSD | Permissive |

No GPL. No AGPL. No copyleft in the storage stack.
