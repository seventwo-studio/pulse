import { Hono } from "hono";
import type { Database } from "bun:sqlite";

interface SyncChangeset {
  id: string;
  parent: string | null;
  snapshot: string;
  timestamp: string;
  author: { kind: string; id: string; session?: string | null };
  message: string;
  files_changed: string[];
  metadata?: unknown;
}

interface SyncSnapshot {
  id: string;
  files: Record<string, string>;
}

interface SyncWorkspace {
  id: string;
  base: string;
  intent: string;
  scope: string[];
  author: { kind: string; id: string; session?: string | null };
  status: string;
  changesets: string[];
}

interface PushBody {
  main: string;
  changesets: SyncChangeset[];
  snapshots: SyncSnapshot[];
  workspaces: SyncWorkspace[];
  files: Record<string, string>; // blob_hash -> base64 content
}

interface PullBody {
  have_main: string | null;
}

/**
 * Format a changeset row from the database into the sync wire format.
 */
function formatChangeset(row: {
  id: string;
  parent: string | null;
  snapshot: string;
  message: string;
  author_id: string;
  author_kind: string;
  author_session: string | null;
  files_changed: string;
  timestamp: string;
  metadata: string | null;
}): SyncChangeset {
  return {
    id: row.id,
    parent: row.parent,
    snapshot: row.snapshot,
    timestamp: row.timestamp,
    author: {
      kind: row.author_kind,
      id: row.author_id,
      ...(row.author_session ? { session: row.author_session } : {}),
    },
    message: row.message,
    files_changed: JSON.parse(row.files_changed),
    ...(row.metadata ? { metadata: JSON.parse(row.metadata) } : {}),
  };
}

export function syncRoutes(db: Database) {
  const app = new Hono();

  // -----------------------------------------------------------------------
  // POST /sync/push — accept a bundle of client-computed objects verbatim
  // -----------------------------------------------------------------------
  app.post("/sync/push", async (c) => {
    const body = (await c.req.json()) as PushBody;

    // Temporarily disable foreign keys for bulk import
    db.exec("PRAGMA foreign_keys = OFF");
    db.exec("BEGIN TRANSACTION");

    try {
      // 1. Store file content as single-chunk blobs
      for (const [blobHash, b64Content] of Object.entries(body.files)) {
        const content = Buffer.from(b64Content, "base64");
        db.query(
          "INSERT OR IGNORE INTO chunks (hash, data, size) VALUES (?, ?, ?)"
        ).run(blobHash, content, content.length);
        db.query("INSERT OR IGNORE INTO blobs (hash) VALUES (?)").run(
          blobHash
        );
        db.query(
          "INSERT OR IGNORE INTO blob_chunks (blob_hash, idx, chunk_hash) VALUES (?, ?, ?)"
        ).run(blobHash, 0, blobHash);
      }

      // 2. Store snapshots
      for (const snapshot of body.snapshots) {
        db.query("INSERT OR IGNORE INTO snapshots (id) VALUES (?)").run(
          snapshot.id
        );
        for (const [path, blobHash] of Object.entries(snapshot.files)) {
          db.query(
            "INSERT OR IGNORE INTO snapshot_files (snapshot_id, path, blob_hash) VALUES (?, ?, ?)"
          ).run(snapshot.id, path, blobHash);
        }
      }

      // 3. Store changesets
      for (const cs of body.changesets) {
        db.query(
          `INSERT OR IGNORE INTO changesets
             (id, parent, snapshot, message, author_id, author_kind, files_changed, timestamp)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)`
        ).run(
          cs.id,
          cs.parent,
          cs.snapshot,
          cs.message,
          cs.author.id,
          cs.author.kind,
          JSON.stringify(cs.files_changed),
          cs.timestamp
        );
      }

      // 4. Store workspaces
      for (const ws of body.workspaces ?? []) {
        db.query(
          "INSERT OR REPLACE INTO workspaces (id, intent, scope, status, base, author_id, author_kind) VALUES (?, ?, ?, ?, ?, ?, ?)"
        ).run(
          ws.id,
          ws.intent,
          JSON.stringify(ws.scope),
          ws.status,
          ws.base,
          ws.author.id,
          ws.author.kind
        );

        // Store workspace changeset associations
        db.query(
          "DELETE FROM workspace_changesets WHERE workspace_id = ?"
        ).run(ws.id);
        for (let i = 0; i < ws.changesets.length; i++) {
          db.query(
            "INSERT OR IGNORE INTO workspace_changesets (workspace_id, changeset_id, idx) VALUES (?, ?, ?)"
          ).run(ws.id, ws.changesets[i], i);
        }
      }

      // 5. Update main
      db.query("INSERT OR REPLACE INTO main (id, head) VALUES (1, ?)").run(
        body.main
      );

      db.exec("COMMIT");
      db.exec("PRAGMA foreign_keys = ON");
    } catch (e) {
      db.exec("ROLLBACK");
      db.exec("PRAGMA foreign_keys = ON");
      throw e;
    }

    return c.json({ main: body.main });
  });

  // -----------------------------------------------------------------------
  // POST /sync/pull — export objects the client doesn't have
  // -----------------------------------------------------------------------
  app.post("/sync/pull", async (c) => {
    const body = (await c.req.json()) as PullBody;
    const haveMain = body.have_main;

    const mainRow = db
      .query("SELECT head FROM main WHERE id = 1")
      .get() as { head: string | null } | null;

    if (!mainRow?.head) {
      return c.json(
        {
          error: {
            code: "repo_not_initialized",
            message: "Repository has not been initialized.",
            status: 400,
          },
        },
        400
      );
    }

    const serverMain = mainRow.head;

    // Walk the changeset chain from main head to the client's known main.
    const changesets: SyncChangeset[] = [];
    const snapshotIds = new Set<string>();
    let current: string | null = serverMain;

    while (current && current !== haveMain) {
      const row = db
        .query("SELECT * FROM changesets WHERE id = ?")
        .get(current) as any;
      if (!row) break;

      // Provide defaults for columns that may not exist
      row.author_session = row.author_session ?? null;
      row.metadata = row.metadata ?? null;

      changesets.push(formatChangeset(row));
      snapshotIds.add(row.snapshot);
      current = row.parent;
    }

    // Reverse so oldest-first (topological order).
    changesets.reverse();

    // Collect snapshots.
    const snapshots: SyncSnapshot[] = [];
    const blobHashes = new Set<string>();

    for (const snapId of snapshotIds) {
      const files = db
        .query(
          "SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?"
        )
        .all(snapId) as { path: string; blob_hash: string }[];

      const fileMap: Record<string, string> = {};
      for (const f of files) {
        fileMap[f.path] = f.blob_hash;
        blobHashes.add(f.blob_hash);
      }

      snapshots.push({ id: snapId as string, files: fileMap });
    }

    // Collect file content.
    const fileContent: Record<string, string> = {};
    for (const blobHash of blobHashes) {
      const chunks = db
        .query(
          "SELECT c.data FROM blob_chunks bc JOIN chunks c ON c.hash = bc.chunk_hash WHERE bc.blob_hash = ? ORDER BY bc.idx"
        )
        .all(blobHash) as { data: Uint8Array }[];

      const totalSize = chunks.reduce((sum, ch) => sum + ch.data.length, 0);
      const content = Buffer.alloc(totalSize);
      let offset = 0;
      for (const chunk of chunks) {
        Buffer.from(chunk.data).copy(content, offset);
        offset += chunk.data.length;
      }
      fileContent[blobHash] = content.toString("base64");
    }

    // Collect workspaces.
    const wsRows = db.query("SELECT * FROM workspaces").all() as any[];
    const workspaces: SyncWorkspace[] = wsRows.map((row) => {
      const csRows = db
        .query(
          "SELECT changeset_id FROM workspace_changesets WHERE workspace_id = ? ORDER BY idx"
        )
        .all(row.id) as { changeset_id: string }[];

      return {
        id: row.id,
        base: row.base,
        intent: row.intent,
        scope: JSON.parse(row.scope),
        author: { kind: row.author_kind, id: row.author_id },
        status: row.status,
        changesets: csRows.map((r) => r.changeset_id),
      };
    });

    return c.json({
      main: serverMain,
      changesets,
      snapshots,
      workspaces,
      files: fileContent,
    });
  });

  return app;
}
