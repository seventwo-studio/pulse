import { Hono } from "hono";
import type { Database } from "bun:sqlite";

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

interface SnapshotFileRow {
  path: string;
  blob_hash: string;
}

function formatChangeset(row: ChangesetRow) {
  return {
    id: row.id,
    parent: row.parent,
    snapshot: row.snapshot,
    message: row.message,
    author: { id: row.author_id, kind: row.author_kind },
    files_changed: JSON.parse(row.files_changed),
    timestamp: row.timestamp,
  };
}

function getMainHead(db: Database): string | null {
  const row = db.query("SELECT head FROM main WHERE id = 1").get() as { head: string | null } | null;
  return row?.head ?? null;
}

export function mainRoutes(db: Database) {
  const app = new Hono();

  // GET /main
  app.get("/main", (c) => {
    const head = getMainHead(db);
    if (!head) {
      return c.json({ error: { code: "repo_not_initialized", message: "Not initialized.", status: 400 } }, 400);
    }

    const row = db.query("SELECT * FROM changesets WHERE id = ?").get(head) as ChangesetRow | null;
    if (!row) {
      return c.json({ error: { code: "internal_error", message: "Main changeset missing.", status: 500 } }, 500);
    }

    return c.json(formatChangeset(row));
  });

  // GET /main/log
  app.get("/main/log", (c) => {
    const limit = Math.min(Number(c.req.query("limit") ?? 50), 1000);
    const author = c.req.query("author");
    const since = c.req.query("since");

    const head = getMainHead(db);
    if (!head) {
      return c.json({ error: { code: "repo_not_initialized", message: "Not initialized.", status: 400 } }, 400);
    }

    // Walk the parent chain
    const results: ReturnType<typeof formatChangeset>[] = [];
    let current: string | null = head;

    while (current && results.length < limit) {
      const row = db.query("SELECT * FROM changesets WHERE id = ?").get(current) as ChangesetRow | null;
      if (!row) break;

      if (author && row.author_id !== author) {
        current = row.parent;
        continue;
      }
      if (since && row.timestamp < since) break;

      results.push(formatChangeset(row));
      current = row.parent;
    }

    return c.json(results);
  });

  // GET /main/snapshot
  app.get("/main/snapshot", (c) => {
    const head = getMainHead(db);
    if (!head) {
      return c.json({ error: { code: "repo_not_initialized", message: "Not initialized.", status: 400 } }, 400);
    }

    const cs = db.query("SELECT snapshot FROM changesets WHERE id = ?").get(head) as { snapshot: string } | null;
    if (!cs) {
      return c.json({ error: { code: "internal_error", message: "Main changeset missing.", status: 500 } }, 500);
    }

    const files = db.query("SELECT path, blob_hash FROM snapshot_files WHERE snapshot_id = ?").all(cs.snapshot) as SnapshotFileRow[];
    const fileMap: Record<string, string> = {};
    for (const f of files) {
      fileMap[f.path] = f.blob_hash;
    }

    return c.json({ id: cs.snapshot, files: fileMap });
  });

  return app;
}
