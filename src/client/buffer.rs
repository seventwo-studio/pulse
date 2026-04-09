use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::primitives::Author;

/// A single buffered commit entry, stored as one JSON line in the buffer file.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BufferEntry {
    pub workspace_id: String,
    /// File path to base64-encoded content.
    pub files: HashMap<String, String>,
    pub message: String,
    pub author: Author,
    pub timestamp: DateTime<Utc>,
}

/// Append-only offline buffer backed by a JSON-lines file.
///
/// Entries are written when the server is unreachable and drained (replayed)
/// once connectivity is restored.
pub struct OfflineBuffer {
    path: PathBuf,
}

impl OfflineBuffer {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Return the default buffer path: `~/.pulse/buffer.jsonl`.
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
        Ok(PathBuf::from(home).join(".pulse").join("buffer.jsonl"))
    }

    /// Ensure the parent directory exists.
    fn ensure_parent(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Append an entry (JSON line) to the buffer file.
    pub fn push(&self, entry: &BufferEntry) -> anyhow::Result<()> {
        self.ensure_parent()?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Read all entries and truncate the file.
    pub fn drain(&self) -> anyhow::Result<Vec<BufferEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: BufferEntry = serde_json::from_str(trimmed)?;
            entries.push(entry);
        }

        // Truncate the file after reading all entries.
        fs::write(&self.path, "")?;

        Ok(entries)
    }

    /// Number of buffered entries.
    pub fn len(&self) -> anyhow::Result<usize> {
        if !self.path.exists() {
            return Ok(0);
        }

        let file = fs::File::open(&self.path)?;
        let reader = BufReader::new(file);
        let count = reader
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.trim().is_empty())
            .count();
        Ok(count)
    }

    pub fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.len()? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::primitives::Author;

    fn make_entry(msg: &str) -> BufferEntry {
        BufferEntry {
            workspace_id: "ws-abcd".into(),
            files: HashMap::from([("src/main.rs".into(), "Y29udGVudA==".into())]),
            message: msg.into(),
            author: Author::human("alice"),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn push_drain_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let buf = OfflineBuffer::new(dir.path().join("buffer.jsonl"));

        let entry = make_entry("first commit");
        buf.push(&entry).unwrap();

        let entries = buf.drain().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "first commit");
        assert_eq!(entries[0].workspace_id, "ws-abcd");
        assert_eq!(entries[0].author, Author::human("alice"));
        assert_eq!(entries[0].files.get("src/main.rs").unwrap(), "Y29udGVudA==");
    }

    #[test]
    fn drain_empty_buffer_returns_empty_vec() {
        let dir = tempfile::tempdir().unwrap();
        let buf = OfflineBuffer::new(dir.path().join("buffer.jsonl"));

        let entries = buf.drain().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn multiple_pushes_single_drain_returns_all_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let buf = OfflineBuffer::new(dir.path().join("buffer.jsonl"));

        buf.push(&make_entry("first")).unwrap();
        buf.push(&make_entry("second")).unwrap();
        buf.push(&make_entry("third")).unwrap();

        let entries = buf.drain().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "first");
        assert_eq!(entries[1].message, "second");
        assert_eq!(entries[2].message, "third");
    }

    #[test]
    fn after_drain_buffer_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let buf = OfflineBuffer::new(dir.path().join("buffer.jsonl"));

        buf.push(&make_entry("one")).unwrap();
        buf.push(&make_entry("two")).unwrap();
        assert_eq!(buf.len().unwrap(), 2);
        assert!(!buf.is_empty().unwrap());

        buf.drain().unwrap();

        assert_eq!(buf.len().unwrap(), 0);
        assert!(buf.is_empty().unwrap());

        let entries = buf.drain().unwrap();
        assert!(entries.is_empty());
    }
}
