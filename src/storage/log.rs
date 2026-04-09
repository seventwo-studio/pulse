// Append-only log with checksummed framing
//
// Frame format: [4-byte LE length][payload][4-byte BLAKE3 checksum]
// - Length = payload byte count (excludes the length and checksum fields)
// - Checksum = first 4 bytes of BLAKE3(payload)

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

const LENGTH_SIZE: usize = 4;
const CHECKSUM_SIZE: usize = 4;
#[derive(Debug, Error)]
pub enum LogError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("checksum mismatch at offset {offset}")]
    ChecksumMismatch { offset: u64 },
    #[error("incomplete entry at offset {offset}")]
    IncompleteEntry { offset: u64 },
}

/// Compute the 4-byte checksum: first 4 bytes of BLAKE3(payload).
fn checksum(payload: &[u8]) -> [u8; 4] {
    let hash = blake3::hash(payload);
    let bytes = hash.as_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

pub struct AppendLog {
    file: File,
    path: PathBuf,
    write_offset: u64,
}

impl AppendLog {
    /// Open or create a log file, running recovery on open.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LogError> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        let write_offset = Self::recover(&mut file)?;

        Ok(Self {
            file,
            path,
            write_offset,
        })
    }

    /// Append payload, return offset where entry starts.
    /// Writes length + payload + checksum, then fsyncs.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, LogError> {
        let offset = self.write_offset;

        // Seek to the write position
        self.file.seek(SeekFrom::Start(offset))?;

        // Write length (4 bytes LE)
        let len = payload.len() as u32;
        self.file.write_all(&len.to_le_bytes())?;

        // Write payload
        self.file.write_all(payload)?;

        // Write checksum (first 4 bytes of BLAKE3)
        let cksum = checksum(payload);
        self.file.write_all(&cksum)?;

        // Fsync to ensure durability
        self.file.sync_all()?;

        self.write_offset = offset + LENGTH_SIZE as u64 + payload.len() as u64 + CHECKSUM_SIZE as u64;

        Ok(offset)
    }

    /// Read single entry at given offset. Validates checksum.
    /// Opens a separate file handle so this can take &self.
    pub fn read_at(&self, offset: u64) -> Result<Vec<u8>, LogError> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;

        // Read length
        let mut len_buf = [0u8; LENGTH_SIZE];
        match file.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(LogError::IncompleteEntry { offset });
            }
            Err(e) => return Err(LogError::Io(e)),
        }
        let payload_len = u32::from_le_bytes(len_buf) as usize;

        // Read payload
        let mut payload = vec![0u8; payload_len];
        match file.read_exact(&mut payload) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(LogError::IncompleteEntry { offset });
            }
            Err(e) => return Err(LogError::Io(e)),
        }

        // Read checksum
        let mut stored_cksum = [0u8; CHECKSUM_SIZE];
        match file.read_exact(&mut stored_cksum) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(LogError::IncompleteEntry { offset });
            }
            Err(e) => return Err(LogError::Io(e)),
        }

        // Validate checksum
        let expected = checksum(&payload);
        if stored_cksum != expected {
            return Err(LogError::ChecksumMismatch { offset });
        }

        Ok(payload)
    }

    /// Iterate all valid entries: yields (offset, payload).
    pub fn iter(&self) -> LogIterator {
        // Open a fresh file handle for iteration
        let file = File::open(&self.path).expect("failed to open log file for iteration");
        let reader = BufReader::new(file);
        LogIterator {
            reader,
            offset: 0,
            file_len: self.write_offset,
        }
    }

    /// Scan from start, truncate any incomplete/corrupt final entry.
    /// Returns the valid file length (where the next write should go).
    fn recover(file: &mut File) -> Result<u64, LogError> {
        let file_len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        let mut offset: u64 = 0;

        loop {
            // Check if we've consumed all bytes
            if offset >= file_len {
                break;
            }

            // Check if there's room for at least a length header
            if file_len - offset < LENGTH_SIZE as u64 {
                // Incomplete length header — truncate here
                break;
            }

            // Read length
            file.seek(SeekFrom::Start(offset))?;
            let mut len_buf = [0u8; LENGTH_SIZE];
            if file.read_exact(&mut len_buf).is_err() {
                break;
            }
            let payload_len = u32::from_le_bytes(len_buf) as u64;

            // Check if the full entry (length + payload + checksum) fits
            let entry_size = LENGTH_SIZE as u64 + payload_len + CHECKSUM_SIZE as u64;
            if offset + entry_size > file_len {
                // Incomplete entry — truncate here
                break;
            }

            // Read payload
            let mut payload = vec![0u8; payload_len as usize];
            if file.read_exact(&mut payload).is_err() {
                break;
            }

            // Read checksum
            let mut stored_cksum = [0u8; CHECKSUM_SIZE];
            if file.read_exact(&mut stored_cksum).is_err() {
                break;
            }

            // Validate checksum
            let expected = checksum(&payload);
            if stored_cksum != expected {
                // Corrupt entry — truncate here
                break;
            }

            // This entry is valid, advance
            offset += entry_size;
        }

        // Truncate file to the last valid offset
        if offset < file_len {
            file.set_len(offset)?;
        }
        file.seek(SeekFrom::Start(offset))?;

        Ok(offset)
    }
}

pub struct LogIterator {
    reader: BufReader<File>,
    offset: u64,
    file_len: u64,
}

impl Iterator for LogIterator {
    type Item = Result<(u64, Vec<u8>), LogError>;

    fn next(&mut self) -> Option<Self::Item> {
        // No more data
        if self.offset >= self.file_len {
            return None;
        }

        let entry_offset = self.offset;

        // Read length
        let mut len_buf = [0u8; LENGTH_SIZE];
        match self.reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Some(Err(LogError::IncompleteEntry {
                    offset: entry_offset,
                }));
            }
            Err(e) => return Some(Err(LogError::Io(e))),
        }
        let payload_len = u32::from_le_bytes(len_buf) as usize;

        // Read payload
        let mut payload = vec![0u8; payload_len];
        match self.reader.read_exact(&mut payload) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Some(Err(LogError::IncompleteEntry {
                    offset: entry_offset,
                }));
            }
            Err(e) => return Some(Err(LogError::Io(e))),
        }

        // Read checksum
        let mut stored_cksum = [0u8; CHECKSUM_SIZE];
        match self.reader.read_exact(&mut stored_cksum) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Some(Err(LogError::IncompleteEntry {
                    offset: entry_offset,
                }));
            }
            Err(e) => return Some(Err(LogError::Io(e))),
        }

        // Validate checksum
        let expected = checksum(&payload);
        if stored_cksum != expected {
            return Some(Err(LogError::ChecksumMismatch {
                offset: entry_offset,
            }));
        }

        self.offset += LENGTH_SIZE as u64 + payload_len as u64 + CHECKSUM_SIZE as u64;

        Some(Ok((entry_offset, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_ten_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let payloads: Vec<Vec<u8>> = (0..10)
            .map(|i| {
                let size = (i + 1) * 100;
                (0..size).map(|b| (b % 256) as u8).collect()
            })
            .collect();

        {
            let mut log = AppendLog::open(&path).unwrap();
            for payload in &payloads {
                log.append(payload).unwrap();
            }
        }

        // Reopen and iterate
        let log = AppendLog::open(&path).unwrap();
        let entries: Vec<(u64, Vec<u8>)> = log.iter().map(|r| r.unwrap()).collect();

        assert_eq!(entries.len(), 10);
        for (i, (_offset, data)) in entries.iter().enumerate() {
            assert_eq!(data, &payloads[i]);
        }
    }

    #[test]
    fn read_at_middle_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let mut log = AppendLog::open(&path).unwrap();
        let _offset0 = log.append(b"first").unwrap();
        let offset1 = log.append(b"second").unwrap();
        let _offset2 = log.append(b"third").unwrap();

        let payload = log.read_at(offset1).unwrap();
        assert_eq!(payload, b"second");
    }

    #[test]
    fn crash_recovery_truncates_partial_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        // Write a valid entry
        {
            let mut log = AppendLog::open(&path).unwrap();
            log.append(b"valid entry").unwrap();
        }

        // Manually append partial garbage: a length header pointing to more
        // data than we actually write
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            // Write a length header claiming 1000 bytes of payload
            file.write_all(&1000u32.to_le_bytes()).unwrap();
            // But only write 5 bytes of "payload"
            file.write_all(b"short").unwrap();
            file.sync_all().unwrap();
        }

        // Reopen — recovery should truncate the garbage
        let log = AppendLog::open(&path).unwrap();
        let entries: Vec<(u64, Vec<u8>)> = log.iter().map(|r| r.unwrap()).collect();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, b"valid entry");
    }

    #[test]
    fn empty_log_iterates_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let log = AppendLog::open(&path).unwrap();
        let entries: Vec<_> = log.iter().collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn checksum_mismatch_on_corruption() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let mut log = AppendLog::open(&path).unwrap();
        let offset = log.append(b"hello world").unwrap();

        // Corrupt a byte in the payload region while the log is still open.
        // We use a separate file handle to write directly, bypassing the log.
        // This simulates bit-rot or storage corruption between append and read.
        {
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            // Seek to the first byte of the payload
            file.seek(SeekFrom::Start(offset + LENGTH_SIZE as u64))
                .unwrap();
            // Overwrite with a different byte
            file.write_all(&[0xFF]).unwrap();
            file.sync_all().unwrap();
        }

        // read_at should detect the corruption
        let result = log.read_at(offset);
        assert!(matches!(result, Err(LogError::ChecksumMismatch { .. })));
    }

    #[test]
    fn large_payload_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        // 1 MB payload
        let payload: Vec<u8> = (0..1_048_576).map(|i| (i % 251) as u8).collect();

        let offset;
        {
            let mut log = AppendLog::open(&path).unwrap();
            offset = log.append(&payload).unwrap();
        }

        let log = AppendLog::open(&path).unwrap();
        let read_back = log.read_at(offset).unwrap();
        assert_eq!(read_back.len(), payload.len());
        assert_eq!(read_back, payload);
    }
}
