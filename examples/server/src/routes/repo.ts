import { Hono } from "hono";
import type { Database } from "bun:sqlite";
import { hash, hashJson } from "../hash";

export function repoRoutes(db: Database) {
  const app = new Hono();

  // POST /repo/init
  app.post("/repo/init", (c) => {
    const main = db.query("SELECT head FROM main WHERE id = 1").get() as
      | { head: string | null }
      | null;

    if (main?.head) {
      return c.json(
        { error: { code: "repo_already_initialized", message: "Repository has already been initialized.", status: 409 } },
        409,
      );
    }

    const emptySnapshotId = hashJson({});
    db.query("INSERT OR IGNORE INTO snapshots (id) VALUES (?)").run(emptySnapshotId);

    const changeset = {
      id: "",
      parent: null,
      snapshot: emptySnapshotId,
      message: "root",
      author_id: "system",
      author_kind: "system",
      files_changed: [] as string[],
      timestamp: new Date().toISOString(),
    };
    changeset.id = hashJson({
      parent: changeset.parent,
      snapshot: changeset.snapshot,
      message: changeset.message,
      author_id: changeset.author_id,
      files_changed: changeset.files_changed,
      timestamp: changeset.timestamp,
    });

    db.query(
      "INSERT INTO changesets (id, parent, snapshot, message, author_id, author_kind, files_changed, timestamp) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    ).run(
      changeset.id,
      changeset.parent,
      changeset.snapshot,
      changeset.message,
      changeset.author_id,
      changeset.author_kind,
      JSON.stringify(changeset.files_changed),
      changeset.timestamp,
    );

    db.query("INSERT OR REPLACE INTO main (id, head) VALUES (1, ?)").run(changeset.id);

    return c.json({ changeset_id: changeset.id, snapshot_id: emptySnapshotId }, 201);
  });

  // GET /repo/status
  app.get("/repo/status", (c) => {
    const main = db.query("SELECT head FROM main WHERE id = 1").get() as
      | { head: string | null }
      | null;

    if (!main?.head) {
      return c.json(
        { error: { code: "repo_not_initialized", message: "Repository has not been initialized.", status: 400 } },
        400,
      );
    }

    const count = db.query("SELECT COUNT(*) as n FROM workspaces WHERE status = 'active'").get() as { n: number };

    return c.json({ main: main.head, active_workspaces: count.n });
  });

  return app;
}
