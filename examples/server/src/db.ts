import { Database } from "bun:sqlite";

export function createDatabase(path: string): Database {
  const db = new Database(path);
  db.exec("PRAGMA journal_mode = WAL");
  db.exec("PRAGMA foreign_keys = ON");
  migrate(db);
  return db;
}

function migrate(db: Database) {
  db.exec(`
    -- Content-addressed chunk storage (raw compressed bytes)
    CREATE TABLE IF NOT EXISTS chunks (
      hash     TEXT PRIMARY KEY,
      data     BLOB NOT NULL,
      size     INTEGER NOT NULL
    );

    -- Blobs: a file broken into ordered chunks
    CREATE TABLE IF NOT EXISTS blobs (
      hash     TEXT PRIMARY KEY
    );

    CREATE TABLE IF NOT EXISTS blob_chunks (
      blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
      idx        INTEGER NOT NULL,
      chunk_hash TEXT NOT NULL REFERENCES chunks(hash),
      PRIMARY KEY (blob_hash, idx)
    );

    -- Snapshots: a point-in-time file manifest
    CREATE TABLE IF NOT EXISTS snapshots (
      id       TEXT PRIMARY KEY,
      created  TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE IF NOT EXISTS snapshot_files (
      snapshot_id TEXT NOT NULL REFERENCES snapshots(id),
      path        TEXT NOT NULL,
      blob_hash   TEXT NOT NULL REFERENCES blobs(hash),
      PRIMARY KEY (snapshot_id, path)
    );

    -- Changesets: a commit on main or a workspace
    CREATE TABLE IF NOT EXISTS changesets (
      id            TEXT PRIMARY KEY,
      parent        TEXT,
      snapshot      TEXT NOT NULL REFERENCES snapshots(id),
      message       TEXT NOT NULL,
      author_id     TEXT NOT NULL,
      author_kind   TEXT NOT NULL,
      files_changed TEXT NOT NULL DEFAULT '[]',
      timestamp     TEXT NOT NULL DEFAULT (datetime('now'))
    );

    -- Workspaces: ephemeral branches
    CREATE TABLE IF NOT EXISTS workspaces (
      id      TEXT PRIMARY KEY,
      intent  TEXT NOT NULL,
      scope   TEXT NOT NULL DEFAULT '[]',
      status  TEXT NOT NULL DEFAULT 'active',
      base    TEXT NOT NULL,
      author_id   TEXT NOT NULL,
      author_kind TEXT NOT NULL,
      created TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE IF NOT EXISTS workspace_changesets (
      workspace_id  TEXT NOT NULL REFERENCES workspaces(id),
      changeset_id  TEXT NOT NULL REFERENCES changesets(id),
      idx           INTEGER NOT NULL,
      PRIMARY KEY (workspace_id, idx)
    );

    -- Main pointer (single row)
    CREATE TABLE IF NOT EXISTS main (
      id    INTEGER PRIMARY KEY CHECK (id = 1),
      head  TEXT REFERENCES changesets(id)
    );
  `);
}
