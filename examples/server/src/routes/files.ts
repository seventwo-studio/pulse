import { Hono } from "hono";
import type { Database } from "bun:sqlite";

export function fileRoutes(db: Database) {
  const app = new Hono();

  // GET /files/*path
  app.get("/files/*", (c) => {
    const path = c.req.path.replace(/^\/files\//, "");
    const snapshotParam = c.req.query("snapshot");

    let snapshotId: string;

    if (snapshotParam) {
      snapshotId = snapshotParam;
    } else {
      const main = db.query("SELECT head FROM main WHERE id = 1").get() as { head: string | null } | null;
      if (!main?.head) {
        return c.json({ error: { code: "repo_not_initialized", message: "Not initialized.", status: 400 } }, 400);
      }
      const cs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(main.head) as { snapshot: string };
      snapshotId = cs.snapshot;
    }

    const file = db.query("SELECT blob_hash FROM snapshot_files WHERE snapshot_id = ? AND path = ?").get(snapshotId, path) as { blob_hash: string } | null;
    if (!file) {
      return c.json({ error: { code: "file_not_found", message: `File '${path}' not found in snapshot ${snapshotId}`, status: 404 } }, 404);
    }

    // Reassemble blob from chunks
    const chunks = db
      .query("SELECT c.data FROM blob_chunks bc JOIN chunks c ON c.hash = bc.chunk_hash WHERE bc.blob_hash = ? ORDER BY bc.idx")
      .all(file.blob_hash) as { data: Uint8Array }[];

    const totalSize = chunks.reduce((sum, c) => sum + c.data.length, 0);
    const result = new Uint8Array(totalSize);
    let offset = 0;
    for (const chunk of chunks) {
      result.set(new Uint8Array(chunk.data), offset);
      offset += chunk.data.length;
    }

    return new Response(result, {
      status: 200,
      headers: { "Content-Type": "application/octet-stream" },
    });
  });

  return app;
}
