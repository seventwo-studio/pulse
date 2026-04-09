// Compact binary codec for Pulse meta objects.
//
// Replaces JSON serialization in the meta logs with a fixed-layout binary
// format. Hashes are stored as raw 32-byte values instead of 64-byte hex
// strings, timestamps as i64 millis instead of ISO strings, etc.
//
// All multi-byte integers are little-endian.
//
// Wire layout per type:
//
//   Blob:
//     [32B hash][u32 chunk_count][32B × chunk_count]
//
//   Snapshot:
//     [32B id][u32 file_count][for each: u16 path_len, path bytes, 32B blob_hash]
//
//   Changeset:
//     [32B id][u8 has_parent][32B parent?][32B snapshot]
//     [i64 timestamp_ms][u8 author_kind][u16 author_id_len][author_id bytes]
//     [u8 has_session][u16 session_len?][session bytes?]
//     [u16 message_len][message bytes]
//     [u16 files_count][for each: u16 path_len, path bytes]
//     [u8 has_metadata][u32 meta_len?][meta JSON bytes?]
//
//   Workspace:
//     [u8 id_len][id bytes][32B base]
//     [u16 intent_len][intent bytes]
//     [u16 scope_count][for each: u16 pattern_len, pattern bytes]
//     [u8 author_kind][u16 author_id_len][author_id bytes]
//     [u8 has_session][u16 session_len?][session bytes?]
//     [u8 status]
//     [u32 changeset_count][32B × changeset_count]

use std::collections::BTreeMap;
use std::io::{self, Read};

use chrono::{TimeZone, Utc};

use crate::core::primitives::*;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("invalid author kind: {0}")]
    InvalidAuthorKind(u8),
    #[error("invalid workspace status: {0}")]
    InvalidWorkspaceStatus(u8),
    #[error("invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("invalid metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Low-level helpers
// ---------------------------------------------------------------------------

fn write_u8(w: &mut Vec<u8>, v: u8) {
    w.push(v);
}

fn write_u16(w: &mut Vec<u8>, v: u16) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn write_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn write_i64(w: &mut Vec<u8>, v: i64) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn write_hash(w: &mut Vec<u8>, h: &Hash) {
    w.extend_from_slice(&h.0);
}

fn write_bytes(w: &mut Vec<u8>, data: &[u8], len_size: LenSize) {
    match len_size {
        LenSize::U16 => write_u16(w, data.len() as u16),
        LenSize::U32 => write_u32(w, data.len() as u32),
    }
    w.extend_from_slice(data);
}

fn write_str(w: &mut Vec<u8>, s: &str, len_size: LenSize) {
    write_bytes(w, s.as_bytes(), len_size);
}

enum LenSize {
    U16,
    U32,
}

fn read_u8(r: &mut &[u8]) -> Result<u8, CodecError> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(buf[0])
}

fn read_u16(r: &mut &[u8]) -> Result<u16, CodecError> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(r: &mut &[u8]) -> Result<u32, CodecError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i64(r: &mut &[u8]) -> Result<i64, CodecError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_hash(r: &mut &[u8]) -> Result<Hash, CodecError> {
    let mut buf = [0u8; 32];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(Hash(buf))
}

fn read_bytes_u16(r: &mut &[u8]) -> Result<Vec<u8>, CodecError> {
    let len = read_u16(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(buf)
}

fn read_bytes_u32(r: &mut &[u8]) -> Result<Vec<u8>, CodecError> {
    let len = read_u32(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(buf)
}

fn read_string_u16(r: &mut &[u8]) -> Result<String, CodecError> {
    let bytes = read_bytes_u16(r)?;
    Ok(String::from_utf8(bytes)?)
}

fn read_string_u8(r: &mut &[u8]) -> Result<String, CodecError> {
    let len = read_u8(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|_| CodecError::UnexpectedEof)?;
    Ok(String::from_utf8(buf)?)
}

// ---------------------------------------------------------------------------
// Author
// ---------------------------------------------------------------------------

fn author_kind_to_u8(kind: &AuthorKind) -> u8 {
    match kind {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::System => 2,
    }
}

fn u8_to_author_kind(v: u8) -> Result<AuthorKind, CodecError> {
    match v {
        0 => Ok(AuthorKind::Human),
        1 => Ok(AuthorKind::Agent),
        2 => Ok(AuthorKind::System),
        _ => Err(CodecError::InvalidAuthorKind(v)),
    }
}

fn encode_author(w: &mut Vec<u8>, author: &Author) {
    write_u8(w, author_kind_to_u8(&author.kind));
    write_str(w, &author.id, LenSize::U16);
    match &author.session {
        Some(s) => {
            write_u8(w, 1);
            write_str(w, s, LenSize::U16);
        }
        None => write_u8(w, 0),
    }
}

fn decode_author(r: &mut &[u8]) -> Result<Author, CodecError> {
    let kind = u8_to_author_kind(read_u8(r)?)?;
    let id = read_string_u16(r)?;
    let has_session = read_u8(r)?;
    let session = if has_session != 0 {
        Some(read_string_u16(r)?)
    } else {
        None
    };
    Ok(Author { kind, id, session })
}

// ---------------------------------------------------------------------------
// Blob
// ---------------------------------------------------------------------------

/// Encode a Blob to compact binary.
pub fn encode_blob(blob: &Blob) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + 4 + blob.chunks.len() * 32);
    write_hash(&mut buf, &blob.hash);
    write_u32(&mut buf, blob.chunks.len() as u32);
    for chunk in &blob.chunks {
        write_hash(&mut buf, chunk);
    }
    buf
}

/// Decode a Blob from compact binary.
pub fn decode_blob(data: &[u8]) -> Result<Blob, CodecError> {
    let mut r = data;
    let hash = read_hash(&mut r)?;
    let count = read_u32(&mut r)? as usize;
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        chunks.push(read_hash(&mut r)?);
    }
    Ok(Blob { hash, chunks })
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Encode a Snapshot to compact binary.
pub fn encode_snapshot(snap: &Snapshot) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + 4 + snap.files.len() * 48);
    write_hash(&mut buf, &snap.id);
    write_u32(&mut buf, snap.files.len() as u32);
    for (path, hash) in &snap.files {
        write_str(&mut buf, path, LenSize::U16);
        write_hash(&mut buf, hash);
    }
    buf
}

/// Decode a Snapshot from compact binary.
pub fn decode_snapshot(data: &[u8]) -> Result<Snapshot, CodecError> {
    let mut r = data;
    let id = read_hash(&mut r)?;
    let count = read_u32(&mut r)? as usize;
    let mut files = BTreeMap::new();
    for _ in 0..count {
        let path = read_string_u16(&mut r)?;
        let hash = read_hash(&mut r)?;
        files.insert(path, hash);
    }
    Ok(Snapshot { id, files })
}

// ---------------------------------------------------------------------------
// Changeset
// ---------------------------------------------------------------------------

/// Encode a Changeset to compact binary.
pub fn encode_changeset(cs: &Changeset) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    write_hash(&mut buf, &cs.id);

    match &cs.parent {
        Some(p) => {
            write_u8(&mut buf, 1);
            write_hash(&mut buf, p);
        }
        None => write_u8(&mut buf, 0),
    }

    write_hash(&mut buf, &cs.snapshot);
    write_i64(&mut buf, cs.timestamp.timestamp_millis());
    encode_author(&mut buf, &cs.author);
    write_str(&mut buf, &cs.message, LenSize::U16);

    write_u16(&mut buf, cs.files_changed.len() as u16);
    for path in &cs.files_changed {
        write_str(&mut buf, path, LenSize::U16);
    }

    match &cs.metadata {
        Some(meta) => {
            write_u8(&mut buf, 1);
            let json = serde_json::to_vec(meta).expect("metadata always serializes");
            write_bytes(&mut buf, &json, LenSize::U32);
        }
        None => write_u8(&mut buf, 0),
    }

    buf
}

/// Decode a Changeset from compact binary.
pub fn decode_changeset(data: &[u8]) -> Result<Changeset, CodecError> {
    let mut r = data;
    let id = read_hash(&mut r)?;

    let has_parent = read_u8(&mut r)?;
    let parent = if has_parent != 0 {
        Some(read_hash(&mut r)?)
    } else {
        None
    };

    let snapshot = read_hash(&mut r)?;
    let timestamp_ms = read_i64(&mut r)?;
    let timestamp = Utc
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .unwrap_or_else(Utc::now);

    let author = decode_author(&mut r)?;
    let message = read_string_u16(&mut r)?;

    let files_count = read_u16(&mut r)? as usize;
    let mut files_changed = Vec::with_capacity(files_count);
    for _ in 0..files_count {
        files_changed.push(read_string_u16(&mut r)?);
    }

    let has_metadata = read_u8(&mut r)?;
    let metadata = if has_metadata != 0 {
        let json_bytes = read_bytes_u32(&mut r)?;
        Some(serde_json::from_slice(&json_bytes)?)
    } else {
        None
    };

    Ok(Changeset {
        id,
        parent,
        snapshot,
        timestamp,
        author,
        message,
        files_changed,
        metadata,
    })
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

fn workspace_status_to_u8(status: &WorkspaceStatus) -> u8 {
    match status {
        WorkspaceStatus::Active => 0,
        WorkspaceStatus::Merged => 1,
        WorkspaceStatus::Abandoned => 2,
    }
}

fn u8_to_workspace_status(v: u8) -> Result<WorkspaceStatus, CodecError> {
    match v {
        0 => Ok(WorkspaceStatus::Active),
        1 => Ok(WorkspaceStatus::Merged),
        2 => Ok(WorkspaceStatus::Abandoned),
        _ => Err(CodecError::InvalidWorkspaceStatus(v)),
    }
}

/// Encode a Workspace to compact binary.
pub fn encode_workspace(ws: &Workspace) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);

    // id is short (ws-XXXX), fits in u8 length
    write_u8(&mut buf, ws.id.len() as u8);
    buf.extend_from_slice(ws.id.as_bytes());

    write_hash(&mut buf, &ws.base);
    write_str(&mut buf, &ws.intent, LenSize::U16);

    write_u16(&mut buf, ws.scope.len() as u16);
    for pattern in &ws.scope {
        write_str(&mut buf, pattern, LenSize::U16);
    }

    encode_author(&mut buf, &ws.author);
    write_u8(&mut buf, workspace_status_to_u8(&ws.status));

    write_u32(&mut buf, ws.changesets.len() as u32);
    for cs in &ws.changesets {
        write_hash(&mut buf, cs);
    }

    buf
}

/// Decode a Workspace from compact binary.
pub fn decode_workspace(data: &[u8]) -> Result<Workspace, CodecError> {
    let mut r = data;
    let id = read_string_u8(&mut r)?;
    let base = read_hash(&mut r)?;
    let intent = read_string_u16(&mut r)?;

    let scope_count = read_u16(&mut r)? as usize;
    let mut scope = Vec::with_capacity(scope_count);
    for _ in 0..scope_count {
        scope.push(read_string_u16(&mut r)?);
    }

    let author = decode_author(&mut r)?;
    let status = u8_to_workspace_status(read_u8(&mut r)?)?;

    let cs_count = read_u32(&mut r)? as usize;
    let mut changesets = Vec::with_capacity(cs_count);
    for _ in 0..cs_count {
        changesets.push(read_hash(&mut r)?);
    }

    Ok(Workspace {
        id,
        base,
        intent,
        scope,
        author,
        status,
        changesets,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn blob_roundtrip() {
        let blob = Blob {
            hash: Hash::from_bytes(b"file content"),
            chunks: vec![
                Hash::from_bytes(b"chunk-0"),
                Hash::from_bytes(b"chunk-1"),
                Hash::from_bytes(b"chunk-2"),
            ],
        };

        let encoded = encode_blob(&blob);
        let decoded = decode_blob(&encoded).unwrap();
        assert_eq!(decoded, blob);

        // Binary should be exactly 32 + 4 + 3*32 = 132 bytes
        assert_eq!(encoded.len(), 132);
    }

    #[test]
    fn blob_empty_chunks() {
        let blob = Blob {
            hash: Hash::from_bytes(b"empty file"),
            chunks: vec![],
        };

        let encoded = encode_blob(&blob);
        let decoded = decode_blob(&encoded).unwrap();
        assert_eq!(decoded, blob);
        assert_eq!(encoded.len(), 36);
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut files = BTreeMap::new();
        files.insert("src/main.rs".into(), Hash::from_bytes(b"main"));
        files.insert("src/lib.rs".into(), Hash::from_bytes(b"lib"));
        files.insert("README.md".into(), Hash::from_bytes(b"readme"));

        let snap = Snapshot::new(files);
        let encoded = encode_snapshot(&snap);
        let decoded = decode_snapshot(&encoded).unwrap();
        assert_eq!(decoded, snap);
    }

    #[test]
    fn snapshot_empty() {
        let snap = Snapshot::empty();
        let encoded = encode_snapshot(&snap);
        let decoded = decode_snapshot(&encoded).unwrap();
        assert_eq!(decoded, snap);
        // 32 (id) + 4 (count=0) = 36 bytes
        assert_eq!(encoded.len(), 36);
    }

    #[test]
    fn changeset_roundtrip_full() {
        let cs = Changeset::new(
            Some(Hash::from_bytes(b"parent")),
            Hash::from_bytes(b"snap"),
            Utc::now(),
            Author::agent("claude-sonnet-4", Some("ws-a7f3".into())),
            "Implement JWT auth".into(),
            vec!["src/auth/jwt.rs".into(), "src/auth/mod.rs".into()],
            Some(serde_json::json!({"reviewed": true})),
        );

        let encoded = encode_changeset(&cs);
        let decoded = decode_changeset(&encoded).unwrap();

        assert_eq!(decoded.id, cs.id);
        assert_eq!(decoded.parent, cs.parent);
        assert_eq!(decoded.snapshot, cs.snapshot);
        // Timestamp loses sub-millisecond precision, compare millis
        assert_eq!(
            decoded.timestamp.timestamp_millis(),
            cs.timestamp.timestamp_millis()
        );
        assert_eq!(decoded.author, cs.author);
        assert_eq!(decoded.message, cs.message);
        assert_eq!(decoded.files_changed, cs.files_changed);
        assert_eq!(decoded.metadata, cs.metadata);
    }

    #[test]
    fn changeset_roundtrip_minimal() {
        let cs = Changeset::new(
            None,
            Hash::from_bytes(b"snap"),
            Utc::now(),
            Author::system(),
            "init".into(),
            vec![],
            None,
        );

        let encoded = encode_changeset(&cs);
        let decoded = decode_changeset(&encoded).unwrap();

        assert_eq!(decoded.id, cs.id);
        assert_eq!(decoded.parent, None);
        assert_eq!(decoded.message, "init");
        assert_eq!(decoded.metadata, None);
    }

    #[test]
    fn workspace_roundtrip() {
        let mut ws = Workspace::new(
            Hash::from_bytes(b"trunk-head"),
            "Add JWT auth".into(),
            vec!["src/auth/*".into(), "src/api/routes.rs".into()],
            Author::agent("claude-sonnet-4", Some("sess-1".into())),
        );
        ws.changesets.push(Hash::from_bytes(b"cs-1"));
        ws.changesets.push(Hash::from_bytes(b"cs-2"));

        let encoded = encode_workspace(&ws);
        let decoded = decode_workspace(&encoded).unwrap();
        assert_eq!(decoded, ws);
    }

    #[test]
    fn workspace_minimal() {
        let ws = Workspace::new(
            Hash::ZERO,
            "quick fix".into(),
            vec![],
            Author::human("luca"),
        );

        let encoded = encode_workspace(&ws);
        let decoded = decode_workspace(&encoded).unwrap();
        assert_eq!(decoded, ws);
    }

    #[test]
    fn blob_size_vs_json() {
        let blob = Blob {
            hash: Hash::from_bytes(b"content"),
            chunks: (0..10)
                .map(|i| Hash::from_bytes(format!("chunk-{}", i).as_bytes()))
                .collect(),
        };

        let binary_size = encode_blob(&blob).len();
        let json_size = serde_json::to_vec(&blob).unwrap().len();

        // Binary: 32 + 4 + 10*32 = 356
        // JSON:   ~780+ bytes (quoted hex strings, keys, brackets)
        assert!(
            binary_size < json_size / 2,
            "binary ({}) should be less than half of JSON ({})",
            binary_size,
            json_size
        );
    }

    #[test]
    fn snapshot_size_vs_json() {
        let mut files = BTreeMap::new();
        for i in 0..100 {
            files.insert(
                format!("src/module_{}/main.rs", i),
                Hash::from_bytes(format!("content-{}", i).as_bytes()),
            );
        }
        let snap = Snapshot::new(files);

        let binary_size = encode_snapshot(&snap).len();
        let json_size = serde_json::to_vec(&snap).unwrap().len();

        // Paths are variable-length strings, so the ratio depends on path length.
        // With short paths (~22 chars), binary saves ~40%. With longer paths the
        // ratio improves further since the fixed 32-byte hash dominates.
        assert!(
            binary_size < json_size,
            "binary ({}) should be smaller than JSON ({})",
            binary_size,
            json_size
        );
    }

    #[test]
    fn decode_truncated_blob_fails() {
        let blob = Blob {
            hash: Hash::from_bytes(b"content"),
            chunks: vec![Hash::from_bytes(b"chunk")],
        };
        let encoded = encode_blob(&blob);

        // Truncate in the middle
        let result = decode_blob(&encoded[..40]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_truncated_changeset_fails() {
        let cs = Changeset::new(
            None,
            Hash::from_bytes(b"snap"),
            Utc::now(),
            Author::human("alice"),
            "test".into(),
            vec![],
            None,
        );
        let encoded = encode_changeset(&cs);

        let result = decode_changeset(&encoded[..20]);
        assert!(result.is_err());
    }
}
