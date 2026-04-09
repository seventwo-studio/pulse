import { Hono } from "hono";
import type { Database } from "bun:sqlite";

interface SnapshotFileRow {
  path: string;
  blob_hash: string;
}

function resolveSnapshot(db: Database, hashHex: string): Record<string, string> | null {
  // Try as changeset first
  const cs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(hashHex) as { snapshot: string } | null;
  const snapshotId = cs ? cs.snapshot : hashHex;

  const exists = db.query("SELECT 1 FROM snapshots WHERE id = ?").get(snapshotId);
  if (!exists) return null;

  const files = db.query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?").all(snapshotId) as SnapshotFileRow[];
  const map: Record<string, string> = {};
  for (const f of files) map[f.path] = f.blob_hash;
  return map;
}

export function diffRoutes(db: Database) {
  const app = new Hono();

  // GET /diff/:a/:b
  app.get("/diff/:a/:b", (c) => {
    const a = c.req.param("a");
    const b = c.req.param("b");

    const snapA = resolveSnapshot(db, a);
    if (!snapA) {
      return c.json({ error: { code: "not_found", message: `Hash ${a} is neither a known changeset nor snapshot`, status: 404 } }, 404);
    }
    const snapB = resolveSnapshot(db, b);
    if (!snapB) {
      return c.json({ error: { code: "not_found", message: `Hash ${b} is neither a known changeset nor snapshot`, status: 404 } }, 404);
    }

    const allPaths = new Set([...Object.keys(snapA), ...Object.keys(snapB)]);
    const added: string[] = [];
    const removed: string[] = [];
    const modified: string[] = [];

    for (const path of allPaths) {
      const inA = path in snapA;
      const inB = path in snapB;
      if (!inA && inB) added.push(path);
      else if (inA && !inB) removed.push(path);
      else if (snapA[path] !== snapB[path]) modified.push(path);
    }

    return c.json({ added, removed, modified });
  });

  return app;
}
