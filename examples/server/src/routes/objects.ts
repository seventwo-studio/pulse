import { Hono } from "hono";
import type { Database } from "bun:sqlite";
import { hash } from "../hash";

/** Simple fixed-size chunking for this example (4 KiB chunks). */
function chunk(data: Uint8Array): Uint8Array[] {
  const CHUNK_SIZE = 4096;
  if (data.length === 0) return [data];
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < data.length; i += CHUNK_SIZE) {
    chunks.push(data.slice(i, i + CHUNK_SIZE));
  }
  return chunks;
}

function storeBlob(db: Database, content: Uint8Array) {
  const chunks = chunk(content);
  const chunkHashes: string[] = [];
  let newChunks = 0;
  let reusedChunks = 0;

  for (const c of chunks) {
    const h = hash(c);
    const existing = db.query("SELECT 1 FROM chunks WHERE hash = ?").get(h);
    if (existing) {
      reusedChunks++;
    } else {
      db.query("INSERT INTO chunks (hash, data, size) VALUES (?, ?, ?)").run(h, c, c.length);
      newChunks++;
    }
    chunkHashes.push(h);
  }

  const blobHash = hash(chunkHashes.join(","));
  db.query("INSERT OR IGNORE INTO blobs (hash) VALUES (?)").run(blobHash);
  for (let i = 0; i < chunkHashes.length; i++) {
    db.query("INSERT OR IGNORE INTO blob_chunks (blob_hash, idx, chunk_hash) VALUES (?, ?, ?)").run(
      blobHash,
      i,
      chunkHashes[i],
    );
  }

  return { blobHash, chunkHashes, newChunks, reusedChunks };
}

export { storeBlob };

export function objectRoutes(db: Database) {
  const app = new Hono();

  // GET /objects/:hash
  app.get("/objects/:hash", (c) => {
    const h = c.req.param("hash");
    const blob = db.query("SELECT hash FROM blobs WHERE hash = ?").get(h) as { hash: string } | null;
    if (!blob) {
      return c.json({ error: { code: "object_not_found", message: `No object with hash ${h}`, status: 404 } }, 404);
    }

    const chunks = db
      .query("SELECT chunk_hash FROM blob_chunks WHERE blob_hash = ? ORDER BY idx")
      .all(h) as { chunk_hash: string }[];

    return c.json({
      type: "blob",
      hash: h,
      chunks: chunks.map((r) => r.chunk_hash),
    });
  });

  // POST /objects (raw body)
  app.post("/objects", async (c) => {
    const body = await c.req.arrayBuffer();
    const content = new Uint8Array(body);
    const result = storeBlob(db, content);

    return c.json(
      {
        hash: result.blobHash,
        chunks: result.chunkHashes,
        new_chunks: result.newChunks,
        reused_chunks: result.reusedChunks,
      },
      201,
    );
  });

  // POST /objects/batch
  app.post("/objects/batch", async (c) => {
    const body = await c.req.json<{ files: Record<string, string> }>();
    const blobs: Record<string, { hash: string; chunks: string[] }> = {};
    let totalNew = 0;
    let totalReused = 0;

    for (const [path, b64] of Object.entries(body.files)) {
      const content = Uint8Array.from(atob(b64), (ch) => ch.charCodeAt(0));
      const result = storeBlob(db, content);
      blobs[path] = { hash: result.blobHash, chunks: result.chunkHashes };
      totalNew += result.newChunks;
      totalReused += result.reusedChunks;
    }

    return c.json({ blobs, stats: { new_chunks: totalNew, reused_chunks: totalReused } }, 201);
  });

  // POST /objects/have
  app.post("/objects/have", async (c) => {
    const body = await c.req.json<{ hashes: string[] }>();
    const have: string[] = [];
    const missing: string[] = [];

    for (const h of body.hashes) {
      const exists = db.query("SELECT 1 FROM chunks WHERE hash = ? UNION SELECT 1 FROM blobs WHERE hash = ?").get(h, h);
      if (exists) have.push(h);
      else missing.push(h);
    }

    return c.json({ have, missing });
  });

  return app;
}
