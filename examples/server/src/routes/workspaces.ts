import { Hono } from "hono";
import type { Database } from "bun:sqlite";
import { hash, hashJson, generateWorkspaceId } from "../hash";
import { storeBlob } from "./objects";

interface WorkspaceRow {
  id: string;
  intent: string;
  scope: string;
  status: string;
  base: string;
  author_id: string;
  author_kind: string;
  created: string;
}

interface ChangesetRow {
  id: string;
  parent: string | null;
  snapshot: string;
  message: string;
  author_id: string;
  author_kind: string;
  files_changed: string;
  timestamp: string;
}

function formatWorkspace(db: Database, row: WorkspaceRow) {
  const csRows = db
    .query("SELECT changeset_id FROM workspace_changesets WHERE workspace_id = ? ORDER BY idx")
    .all(row.id) as { changeset_id: string }[];

  return {
    id: row.id,
    intent: row.intent,
    scope: JSON.parse(row.scope),
    status: row.status,
    base: row.base,
    author: { id: row.author_id, kind: row.author_kind },
    changesets: csRows.map((r) => r.changeset_id),
    created: row.created,
  };
}

function getLatestSnapshot(db: Database, workspaceId: string): string {
  // Get the last changeset's snapshot, or fall back to the base changeset's snapshot
  const lastCs = db
    .query(
      "SELECT c.snapshot FROM workspace_changesets wc JOIN changesets c ON c.id = wc.changeset_id WHERE wc.workspace_id = ? ORDER BY wc.idx DESC LIMIT 1",
    )
    .get(workspaceId) as { snapshot: string } | null;

  if (lastCs) return lastCs.snapshot;

  const ws = db.query("SELECT base FROM workspaces WHERE id = ?").get(workspaceId) as { base: string };
  const baseCs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(ws.base) as { snapshot: string };
  return baseCs.snapshot;
}

export function workspaceRoutes(db: Database) {
  const app = new Hono();

  // POST /workspaces
  app.post("/workspaces", async (c) => {
    const body = await c.req.json<{ intent: string; scope: string[]; author: { id: string; kind: string } }>();

    const trunk = db.query("SELECT head FROM trunk WHERE id = 1").get() as { head: string | null } | null;
    if (!trunk?.head) {
      return c.json({ error: { code: "repo_not_initialized", message: "Not initialized.", status: 400 } }, 400);
    }

    const id = generateWorkspaceId();
    db.query(
      "INSERT INTO workspaces (id, intent, scope, base, author_id, author_kind) VALUES (?, ?, ?, ?, ?, ?)",
    ).run(id, body.intent, JSON.stringify(body.scope), trunk.head, body.author.id, body.author.kind);

    const ws = db.query("SELECT * FROM workspaces WHERE id = ?").get(id) as WorkspaceRow;

    return c.json({ workspace: formatWorkspace(db, ws), overlaps: [] }, 201);
  });

  // GET /workspaces
  app.get("/workspaces", (c) => {
    const all = c.req.query("all") === "true";
    const rows = all
      ? (db.query("SELECT * FROM workspaces").all() as WorkspaceRow[])
      : (db.query("SELECT * FROM workspaces WHERE status = 'active'").all() as WorkspaceRow[]);

    return c.json(rows.map((r) => formatWorkspace(db, r)));
  });

  // GET /workspaces/:id
  app.get("/workspaces/:id", (c) => {
    const id = c.req.param("id");
    const ws = db.query("SELECT * FROM workspaces WHERE id = ?").get(id) as WorkspaceRow | null;
    if (!ws) {
      return c.json({ error: { code: "workspace_not_found", message: `Workspace ${id} not found.`, status: 404 } }, 404);
    }
    return c.json(formatWorkspace(db, ws));
  });

  // POST /workspaces/:id/commit
  app.post("/workspaces/:id/commit", async (c) => {
    const id = c.req.param("id");
    const ws = db.query("SELECT * FROM workspaces WHERE id = ? AND status = 'active'").get(id) as WorkspaceRow | null;
    if (!ws) {
      return c.json({ error: { code: "workspace_not_found", message: `Workspace ${id} not found or not active.`, status: 404 } }, 404);
    }

    const body = await c.req.json<{
      files: Record<string, string>;
      message: string;
      author: { id: string; kind: string };
    }>();

    // Store blobs for each file
    const fileBlobs: Record<string, string> = {};
    let totalNew = 0;
    let totalReused = 0;

    for (const [path, b64] of Object.entries(body.files)) {
      const content = Uint8Array.from(atob(b64), (ch) => ch.charCodeAt(0));
      const result = storeBlob(db, content);
      fileBlobs[path] = result.blobHash;
      totalNew += result.newChunks;
      totalReused += result.reusedChunks;
    }

    // Build new snapshot from previous + changed files
    const prevSnapshotId = getLatestSnapshot(db, id);
    const prevFiles = db
      .query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?")
      .all(prevSnapshotId) as { path: string; blob_hash: string }[];

    const newFiles: Record<string, string> = {};
    for (const f of prevFiles) newFiles[f.path] = f.blob_hash;
    for (const [path, blobHash] of Object.entries(fileBlobs)) newFiles[path] = blobHash;

    // Compute snapshot id deterministically
    const snapshotId = hashJson(newFiles);
    db.query("INSERT OR IGNORE INTO snapshots (id) VALUES (?)").run(snapshotId);
    for (const [path, blobHash] of Object.entries(newFiles)) {
      db.query("INSERT OR IGNORE INTO snapshot_files (snapshot_id, path, blob_hash) VALUES (?, ?, ?)").run(
        snapshotId,
        path,
        blobHash,
      );
    }

    // Find parent changeset
    const csCount = db
      .query("SELECT COUNT(*) as n FROM workspace_changesets WHERE workspace_id = ?")
      .get(id) as { n: number };
    const parentCs =
      csCount.n > 0
        ? (
            db
              .query(
                "SELECT changeset_id FROM workspace_changesets WHERE workspace_id = ? ORDER BY idx DESC LIMIT 1",
              )
              .get(id) as { changeset_id: string }
          ).changeset_id
        : ws.base;

    // Create changeset
    const filesChanged = Object.keys(body.files);
    const timestamp = new Date().toISOString();
    const changesetId = hashJson({
      parent: parentCs,
      snapshot: snapshotId,
      message: body.message,
      author_id: body.author.id,
      files_changed: filesChanged,
      timestamp,
    });

    db.query(
      "INSERT INTO changesets (id, parent, snapshot, message, author_id, author_kind, files_changed, timestamp) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    ).run(changesetId, parentCs, snapshotId, body.message, body.author.id, body.author.kind, JSON.stringify(filesChanged), timestamp);

    db.query("INSERT INTO workspace_changesets (workspace_id, changeset_id, idx) VALUES (?, ?, ?)").run(
      id,
      changesetId,
      csCount.n,
    );

    return c.json(
      {
        changeset: {
          id: changesetId,
          parent: parentCs,
          snapshot: snapshotId,
          message: body.message,
          author: { id: body.author.id, kind: body.author.kind },
          files_changed: filesChanged,
          timestamp,
        },
        stats: { new_chunks: totalNew, reused_chunks: totalReused },
      },
      201,
    );
  });

  // POST /workspaces/:id/merge
  app.post("/workspaces/:id/merge", (c) => {
    const id = c.req.param("id");
    const ws = db.query("SELECT * FROM workspaces WHERE id = ? AND status = 'active'").get(id) as WorkspaceRow | null;
    if (!ws) {
      return c.json({ error: { code: "workspace_not_found", message: `Workspace ${id} not found or not active.`, status: 404 } }, 404);
    }

    const trunk = db.query("SELECT head FROM trunk WHERE id = 1").get() as { head: string };

    // Get workspace's latest snapshot
    const wsSnapshotId = getLatestSnapshot(db, id);

    // Get trunk's current snapshot
    const trunkCs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(trunk.head) as { snapshot: string };
    const trunkSnapshotId = trunkCs.snapshot;

    // Get base snapshot
    const baseCs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(ws.base) as { snapshot: string };

    // Check for conflicts: files modified in both trunk and workspace since the base
    const trunkFiles = db.query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?").all(trunkSnapshotId) as { path: string; blob_hash: string }[];
    const wsFiles = db.query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?").all(wsSnapshotId) as { path: string; blob_hash: string }[];
    const baseFiles = db.query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?").all(baseCs.snapshot) as { path: string; blob_hash: string }[];

    const toMap = (rows: { path: string; blob_hash: string }[]) => {
      const m: Record<string, string> = {};
      for (const r of rows) m[r.path] = r.blob_hash;
      return m;
    };
    const trunkMap = toMap(trunkFiles);
    const wsMap = toMap(wsFiles);
    const baseMap = toMap(baseFiles);

    // Detect conflicts
    const conflicts: string[] = [];
    for (const path of Object.keys(wsMap)) {
      const wsChanged = wsMap[path] !== baseMap[path];
      const trunkChanged = trunkMap[path] !== undefined && trunkMap[path] !== baseMap[path];
      if (wsChanged && trunkChanged && wsMap[path] !== trunkMap[path]) {
        conflicts.push(path);
      }
    }

    if (conflicts.length > 0) {
      return c.json(
        {
          error: { code: "merge_conflict", message: `Merge conflict in workspace ${id}`, status: 409 },
          conflicting_files: conflicts,
          trunk_snapshot: trunkSnapshotId,
          workspace_snapshot: wsSnapshotId,
        },
        409,
      );
    }

    // Fast-forward or merge: apply workspace changes on top of trunk
    const mergedFiles = { ...trunkMap };
    for (const [path, blobHash] of Object.entries(wsMap)) {
      if (blobHash !== baseMap[path]) {
        mergedFiles[path] = blobHash;
      }
    }

    const mergedSnapshotId = hashJson(mergedFiles);
    db.query("INSERT OR IGNORE INTO snapshots (id) VALUES (?)").run(mergedSnapshotId);
    for (const [path, blobHash] of Object.entries(mergedFiles)) {
      db.query("INSERT OR IGNORE INTO snapshot_files (snapshot_id, path, blob_hash) VALUES (?, ?, ?)").run(
        mergedSnapshotId,
        path,
        blobHash,
      );
    }

    // Create merge changeset
    const timestamp = new Date().toISOString();
    const filesChanged = Object.keys(wsMap).filter((p) => wsMap[p] !== baseMap[p]);
    const changesetId = hashJson({
      parent: trunk.head,
      snapshot: mergedSnapshotId,
      message: `Merge workspace ${id}`,
      author_id: ws.author_id,
      files_changed: filesChanged,
      timestamp,
    });

    db.query(
      "INSERT INTO changesets (id, parent, snapshot, message, author_id, author_kind, files_changed, timestamp) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    ).run(changesetId, trunk.head, mergedSnapshotId, `Merge workspace ${id}`, ws.author_id, ws.author_kind, JSON.stringify(filesChanged), timestamp);

    // Advance trunk
    db.query("UPDATE trunk SET head = ? WHERE id = 1").run(changesetId);

    // Mark workspace as merged
    db.query("UPDATE workspaces SET status = 'merged' WHERE id = ?").run(id);

    return c.json({
      changeset: {
        id: changesetId,
        parent: trunk.head,
        snapshot: mergedSnapshotId,
        message: `Merge workspace ${id}`,
        author: { id: ws.author_id, kind: ws.author_kind },
        files_changed: filesChanged,
        timestamp,
      },
    });
  });

  // DELETE /workspaces/:id
  app.delete("/workspaces/:id", (c) => {
    const id = c.req.param("id");
    const ws = db.query("SELECT * FROM workspaces WHERE id = ? AND status = 'active'").get(id) as WorkspaceRow | null;
    if (!ws) {
      return c.json({ error: { code: "workspace_not_found", message: `Workspace ${id} not found or not active.`, status: 404 } }, 404);
    }

    db.query("UPDATE workspaces SET status = 'abandoned' WHERE id = ?").run(id);

    return c.json({ workspace: formatWorkspace(db, { ...ws, status: "abandoned" }) });
  });

  return app;
}
