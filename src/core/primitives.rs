use std::collections::BTreeMap;
use std::fmt;
use std::hash;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

/// BLAKE3 hash, 32 bytes. Displayed and parsed as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// All-zero hash, useful in tests.
    #[allow(dead_code)]
    pub const ZERO: Self = Self([0u8; 32]);

    /// Compute the BLAKE3 hash of arbitrary bytes.
    pub fn from_bytes(data: &[u8]) -> Self {
        let h = blake3::hash(data);
        Self(*h.as_bytes())
    }

    /// Wrap an existing 32-byte array.
    #[allow(dead_code)]
    pub fn from_slice(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    /// Borrow the inner bytes.
    #[allow(dead_code)]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex: String = self.0.iter().take(4).map(|b| format!("{b:02x}")).collect();
        write!(f, "Hash({hex})")
    }
}

impl FromStr for Hash {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64 {
            return Err(HashParseError::InvalidLength(s.len()));
        }
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            bytes[i] =
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(HashParseError::Hex)?;
        }
        Ok(Self(bytes))
    }
}

impl hash::Hash for Hash {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Serialize Hash as a hex string in JSON.
impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Deserialize Hash from a hex string in JSON.
impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Error returned when parsing a hex string into a [`Hash`].
#[derive(Debug, thiserror::Error)]
pub enum HashParseError {
    #[error("expected 64 hex characters, got {0}")]
    InvalidLength(usize),
    #[error("invalid hex: {0}")]
    Hex(std::num::ParseIntError),
}

// ---------------------------------------------------------------------------
// Author
// ---------------------------------------------------------------------------

/// Author of a changeset or workspace action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Author {
    pub kind: AuthorKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// What kind of actor produced the change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    Human,
    Agent,
    System,
}

impl Author {
    /// The built-in system author used for automatic operations.
    pub fn system() -> Self {
        Self {
            kind: AuthorKind::System,
            id: "pulse".into(),
            session: None,
        }
    }

    /// Convenience constructor for a human author.
    pub fn human(id: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Human,
            id: id.into(),
            session: None,
        }
    }

    /// Convenience constructor for an AI-agent author.
    #[allow(dead_code)]
    pub fn agent(id: impl Into<String>, session: Option<String>) -> Self {
        Self {
            kind: AuthorKind::Agent,
            id: id.into(),
            session,
        }
    }
}

// ---------------------------------------------------------------------------
// Blob
// ---------------------------------------------------------------------------

/// Ordered list of chunk hashes representing file content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Blob {
    pub hash: Hash,
    pub chunks: Vec<Hash>,
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Complete project state — flat path-to-blob-hash map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub id: Hash,
    /// Path to blob hash. BTreeMap for deterministic ordering.
    pub files: BTreeMap<String, Hash>,
}

impl Snapshot {
    /// An empty snapshot (no files). The id is the BLAKE3 hash of `{}`.
    pub fn empty() -> Self {
        let files = BTreeMap::new();
        let id = Self::compute_id(&files);
        Self { id, files }
    }

    /// Compute a deterministic id by hashing the canonical JSON of the files map.
    pub fn compute_id(files: &BTreeMap<String, Hash>) -> Hash {
        let json = serde_json::to_vec(files).expect("BTreeMap<String, Hash> always serializes");
        Hash::from_bytes(&json)
    }

    /// Create a new snapshot from a file map, computing the id automatically.
    pub fn new(files: BTreeMap<String, Hash>) -> Self {
        let id = Self::compute_id(&files);
        Self { id, files }
    }
}

// ---------------------------------------------------------------------------
// Changeset
// ---------------------------------------------------------------------------

/// Transition between two snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Changeset {
    pub id: Hash,
    pub parent: Option<Hash>,
    pub snapshot: Hash,
    pub timestamp: DateTime<Utc>,
    pub author: Author,
    pub message: String,
    pub files_changed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Changeset {
    /// Compute a deterministic changeset id from all fields (excluding `id` itself).
    pub fn compute_id(
        parent: &Option<Hash>,
        snapshot: &Hash,
        timestamp: &DateTime<Utc>,
        author: &Author,
        message: &str,
        files_changed: &[String],
        metadata: &Option<serde_json::Value>,
    ) -> Hash {
        let canonical = serde_json::json!({
            "parent": parent,
            "snapshot": snapshot,
            "timestamp": timestamp,
            "author": author,
            "message": message,
            "files_changed": files_changed,
            "metadata": metadata,
        });
        let bytes = serde_json::to_vec(&canonical).expect("json! value always serializes");
        Hash::from_bytes(&bytes)
    }

    /// Create a new changeset, computing the id from the provided fields.
    pub fn new(
        parent: Option<Hash>,
        snapshot: Hash,
        timestamp: DateTime<Utc>,
        author: Author,
        message: String,
        files_changed: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        let id =
            Self::compute_id(&parent, &snapshot, &timestamp, &author, &message, &files_changed, &metadata);
        Self {
            id,
            parent,
            snapshot,
            timestamp,
            author,
            message,
            files_changed,
            metadata,
        }
    }

    /// Create the root changeset that initialises a repository.
    pub fn root(snapshot: Hash) -> Self {
        Self::new(
            None,
            snapshot,
            Utc::now(),
            Author::system(),
            "Repository initialized".into(),
            vec![],
            None,
        )
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

/// Workspace status lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Active,
    Merged,
    Abandoned,
}

/// Ephemeral workspace for isolated work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    /// Identifier in `ws-XXXX` format.
    pub id: String,
    /// Trunk head hash at workspace creation time.
    pub base: Hash,
    /// Human-readable description of the workspace's purpose.
    pub intent: String,
    /// Glob patterns scoping the workspace.
    pub scope: Vec<String>,
    pub author: Author,
    pub status: WorkspaceStatus,
    pub changesets: Vec<Hash>,
}

impl Workspace {
    /// Generate a workspace id: `ws-` followed by 4 hex characters from a UUID v4.
    pub fn generate_id() -> String {
        let uuid = uuid::Uuid::new_v4();
        let hex = uuid.as_simple().to_string();
        format!("ws-{}", &hex[..4])
    }

    /// Create a new active workspace with no changesets.
    pub fn new(base: Hash, intent: String, scope: Vec<String>, author: Author) -> Self {
        Self {
            id: Self::generate_id(),
            base,
            intent,
            scope,
            author,
            status: WorkspaceStatus::Active,
            changesets: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Hash: display as hex, parse from hex, roundtrip
    #[test]
    fn hash_display_parse_roundtrip() {
        let h = Hash::from_bytes(b"hello world");
        let hex = h.to_string();
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));

        let parsed: Hash = hex.parse().unwrap();
        assert_eq!(h, parsed);
    }

    // 2. Hash::from_bytes: same input -> same hash, different input -> different hash
    #[test]
    fn hash_from_bytes_deterministic() {
        let a = Hash::from_bytes(b"same");
        let b = Hash::from_bytes(b"same");
        let c = Hash::from_bytes(b"different");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // 3. Snapshot hashing: same files -> same id; different files -> different id
    #[test]
    fn snapshot_hashing() {
        let mut files = BTreeMap::new();
        files.insert("a.rs".into(), Hash::from_bytes(b"content a"));

        let s1 = Snapshot::new(files.clone());
        let s2 = Snapshot::new(files.clone());
        assert_eq!(s1.id, s2.id);

        files.insert("b.rs".into(), Hash::from_bytes(b"content b"));
        let s3 = Snapshot::new(files);
        assert_ne!(s1.id, s3.id);
    }

    // 4. Snapshot::empty: id is deterministic
    #[test]
    fn snapshot_empty_deterministic() {
        let a = Snapshot::empty();
        let b = Snapshot::empty();
        assert_eq!(a.id, b.id);
        assert!(a.files.is_empty());
    }

    // 5. Changeset hashing: deterministic given same inputs; different inputs -> different id
    #[test]
    fn changeset_hashing() {
        let ts = Utc::now();
        let snap = Hash::from_bytes(b"snap");
        let author = Author::human("alice");

        let c1 = Changeset::new(
            None,
            snap,
            ts,
            author.clone(),
            "first".into(),
            vec!["a.rs".into()],
            None,
        );
        let c2 = Changeset::new(
            None,
            snap,
            ts,
            author.clone(),
            "first".into(),
            vec!["a.rs".into()],
            None,
        );
        assert_eq!(c1.id, c2.id);

        let c3 = Changeset::new(
            None,
            snap,
            ts,
            author,
            "second".into(),
            vec!["a.rs".into()],
            None,
        );
        assert_ne!(c1.id, c3.id);
    }

    // 6. Changeset serialization roundtrip: serialize to JSON, deserialize, compare
    #[test]
    fn changeset_serde_roundtrip() {
        let cs = Changeset::new(
            Some(Hash::from_bytes(b"parent")),
            Hash::from_bytes(b"snap"),
            Utc::now(),
            Author::agent("claude", Some("sess-1".into())),
            "implement feature".into(),
            vec!["src/lib.rs".into()],
            Some(serde_json::json!({"key": "value"})),
        );

        let json = serde_json::to_string(&cs).unwrap();
        let deserialized: Changeset = serde_json::from_str(&json).unwrap();
        assert_eq!(cs, deserialized);
    }

    // 7. Workspace::generate_id: starts with "ws-", 7 chars total
    #[test]
    fn workspace_generate_id_format() {
        let id = Workspace::generate_id();
        assert!(id.starts_with("ws-"));
        assert_eq!(id.len(), 7);
        assert!(id[3..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    // 8. Workspace::new: status is Active, changesets is empty
    #[test]
    fn workspace_new_defaults() {
        let ws = Workspace::new(
            Hash::ZERO,
            "add logging".into(),
            vec!["src/**".into()],
            Author::human("bob"),
        );
        assert_eq!(ws.status, WorkspaceStatus::Active);
        assert!(ws.changesets.is_empty());
        assert!(ws.id.starts_with("ws-"));
    }

    // 9. Author convenience constructors: system(), human(), agent()
    #[test]
    fn author_constructors() {
        let sys = Author::system();
        assert_eq!(sys.kind, AuthorKind::System);
        assert_eq!(sys.id, "pulse");
        assert!(sys.session.is_none());

        let human = Author::human("alice");
        assert_eq!(human.kind, AuthorKind::Human);
        assert_eq!(human.id, "alice");
        assert!(human.session.is_none());

        let agent = Author::agent("claude", Some("s-123".into()));
        assert_eq!(agent.kind, AuthorKind::Agent);
        assert_eq!(agent.id, "claude");
        assert_eq!(agent.session.as_deref(), Some("s-123"));
    }

    // 10. WorkspaceStatus serialization: "active", "merged", "abandoned" in JSON
    #[test]
    fn workspace_status_serde() {
        let active = serde_json::to_string(&WorkspaceStatus::Active).unwrap();
        assert_eq!(active, "\"active\"");

        let merged = serde_json::to_string(&WorkspaceStatus::Merged).unwrap();
        assert_eq!(merged, "\"merged\"");

        let abandoned = serde_json::to_string(&WorkspaceStatus::Abandoned).unwrap();
        assert_eq!(abandoned, "\"abandoned\"");

        // Roundtrip
        let parsed: WorkspaceStatus = serde_json::from_str(&active).unwrap();
        assert_eq!(parsed, WorkspaceStatus::Active);
    }

    // Additional: Hash Debug shows first 8 hex chars
    #[test]
    fn hash_debug_short() {
        let h = Hash::ZERO;
        let dbg = format!("{h:?}");
        assert_eq!(dbg, "Hash(00000000)");
    }

    // Additional: Hash ZERO constant
    #[test]
    fn hash_zero() {
        assert_eq!(Hash::ZERO.0, [0u8; 32]);
    }

    // Additional: Hash parse errors
    #[test]
    fn hash_parse_invalid_length() {
        let result = "abcd".parse::<Hash>();
        assert!(result.is_err());
    }

    #[test]
    fn hash_parse_invalid_hex() {
        let result = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
            .parse::<Hash>();
        assert!(result.is_err());
    }

    // Additional: Hash serializes as hex string (not byte array) in JSON
    #[test]
    fn hash_json_is_hex_string() {
        let h = Hash::ZERO;
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(
            json,
            "\"0000000000000000000000000000000000000000000000000000000000000000\""
        );
        let parsed: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, h);
    }
}
