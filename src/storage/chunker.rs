// Structural chunker with FastCDC fallback

use fastcdc::v2020::FastCDC;

pub const CHUNK_MIN: usize = 512;
pub const CHUNK_TARGET: usize = 4096;
pub const CHUNK_MAX: usize = 16384;

/// Maximum bytes scanned for binary detection.
const BINARY_SCAN_LIMIT: usize = 8192;

/// Split content into structural chunks.
///
/// Text content is split using a two-pass structural algorithm that detects
/// language-level boundaries (functions, classes, imports, etc.). Binary
/// content falls back to FastCDC directly.
pub fn chunk(content: &[u8]) -> Vec<Vec<u8>> {
    if content.is_empty() {
        return Vec::new();
    }

    if is_binary(content) {
        return fastcdc_chunk(content);
    }

    let boundaries = score_boundaries(content);
    split_at_boundaries(content, &boundaries)
}

/// Score boundaries in text content. Returns (byte_offset, score) pairs.
/// byte_offset is the offset of the start of the line following the boundary.
fn score_boundaries(content: &[u8]) -> Vec<(usize, u8)> {
    let lines = split_lines(content);
    let mut boundaries: Vec<(usize, u8)> = Vec::new();

    for i in 1..lines.len() {
        let prev = &lines[i - 1];
        let curr = &lines[i];
        let byte_offset = curr.start;

        let mut score: u8 = 0;

        // Double empty line: two consecutive empty/whitespace-only lines
        // Check if previous line AND the one before it are both empty
        if i >= 2
            && is_empty_or_whitespace_line(content, prev)
            && is_empty_or_whitespace_line(content, &lines[i - 2])
        {
            score = score.max(5);
        } else if is_empty_or_whitespace_line(content, prev) {
            // Single empty line (the boundary is after the empty line)
            score = score.max(3);
        }

        let curr_text = &content[curr.start..curr.end];
        let prev_text = &content[prev.start..prev.end];

        let curr_indent = leading_indent(curr_text);
        let prev_indent = leading_indent(prev_text);
        let curr_stripped = strip_leading_whitespace(curr_text);

        // Top-level declaration (indent <= 1 level, i.e., 0-4 spaces or 0-1 tab)
        if curr_indent <= 1 && is_top_level_declaration(curr_stripped) {
            score = score.max(4);
        }

        // Decorator/attribute
        if curr_indent <= 1
            && (curr_stripped.starts_with(b"#[") || curr_stripped.starts_with(b"@"))
        {
            score = score.max(4);
        }

        // Closing brace at indent <= 1
        if curr_indent <= 1 && is_closing_brace(curr_stripped) {
            score = score.max(3);
        }

        // Comment block start
        if is_comment_block_start(curr_stripped, curr_indent) {
            score = score.max(2);
        }

        // Indentation drop
        if curr_indent < prev_indent && !is_empty_or_whitespace(curr_text) {
            score = score.max(2);
        }

        // Import block edge
        let curr_is_import = is_import_line(curr_stripped);
        let prev_stripped = strip_leading_whitespace(prev_text);
        let prev_is_import = is_import_line(prev_stripped);
        if curr_is_import != prev_is_import {
            score = score.max(2);
        }

        if score > 0 {
            boundaries.push((byte_offset, score));
        }
    }

    boundaries
}

/// Check if content appears to be binary.
fn is_binary(content: &[u8]) -> bool {
    let limit = content.len().min(BINARY_SCAN_LIMIT);
    content[..limit].contains(&0x00)
}

/// FastCDC fallback for a segment.
fn fastcdc_chunk(content: &[u8]) -> Vec<Vec<u8>> {
    let chunker = FastCDC::new(content, CHUNK_MIN as u32, CHUNK_TARGET as u32, CHUNK_MAX as u32);
    chunker
        .map(|c| content[c.offset..c.offset + c.length].to_vec())
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A line span: [start, end) in the content buffer. Does NOT include the
/// trailing newline character(s).
struct LineSpan {
    start: usize,
    end: usize,
}

/// Split content into line spans. Each span covers [start, end) of the line
/// text (excluding `\n` or `\r\n`).
fn split_lines(content: &[u8]) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut start = 0;

    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            let end = if i > start && content[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            lines.push(LineSpan { start, end });
            start = i + 1;
        }
    }

    // Trailing content without a final newline
    if start < content.len() {
        lines.push(LineSpan {
            start,
            end: content.len(),
        });
    }

    lines
}

/// Returns the indent level: number of "indent units" (4 spaces = 1, 1 tab = 1).
fn leading_indent(line: &[u8]) -> usize {
    let mut spaces = 0usize;
    let mut tabs = 0usize;
    for &b in line {
        match b {
            b' ' => spaces += 1,
            b'\t' => tabs += 1,
            _ => break,
        }
    }
    tabs + spaces / 4
}

fn strip_leading_whitespace(line: &[u8]) -> &[u8] {
    let pos = line
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(line.len());
    &line[pos..]
}

fn is_empty_or_whitespace(line: &[u8]) -> bool {
    line.iter().all(|&b| b == b' ' || b == b'\t' || b == b'\r')
}

fn is_empty_or_whitespace_line(content: &[u8], span: &LineSpan) -> bool {
    is_empty_or_whitespace(&content[span.start..span.end])
}

/// Check if a stripped line starts with a top-level declaration keyword.
fn is_top_level_declaration(stripped: &[u8]) -> bool {
    const KEYWORDS: &[&[u8]] = &[
        b"fn ",
        b"func ",
        b"def ",
        b"class ",
        b"struct ",
        b"enum ",
        b"impl ",
        b"interface ",
        b"type ",
        b"const ",
        b"let ",
        b"var ",
        b"export ",
        b"pub fn ",
        b"pub struct ",
        b"pub enum ",
        b"pub const ",
        b"pub type ",
        b"pub mod ",
        b"pub trait ",
        b"trait ",
        b"mod ",
    ];
    KEYWORDS.iter().any(|kw| stripped.starts_with(kw))
}

/// Closing brace: trimmed line is `}` or `};`
fn is_closing_brace(stripped: &[u8]) -> bool {
    stripped == b"}" || stripped == b"};"
}

/// Comment block start: `///`, `/**`, `# ` (Python, indent 0 only), `"""`
fn is_comment_block_start(stripped: &[u8], indent: usize) -> bool {
    if stripped.starts_with(b"///") || stripped.starts_with(b"/**") || stripped.starts_with(b"\"\"\"")
    {
        return true;
    }
    // Python comment at indent 0
    if indent == 0 && stripped.starts_with(b"# ") {
        return true;
    }
    false
}

/// Import/use line detection.
fn is_import_line(stripped: &[u8]) -> bool {
    stripped.starts_with(b"import ")
        || stripped.starts_with(b"use ")
        || stripped.starts_with(b"require(")
        || stripped.starts_with(b"from ")
}

/// Pass 2: walk content and split at scored boundaries.
fn split_at_boundaries(content: &[u8], boundaries: &[(usize, u8)]) -> Vec<Vec<u8>> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut chunk_start: usize = 0;
    let mut boundary_idx: usize = 0;

    // Walk through boundaries in order
    while chunk_start < content.len() {
        let mut split_at: Option<usize> = None;

        // Search for a suitable boundary
        while boundary_idx < boundaries.len() {
            let (offset, score) = boundaries[boundary_idx];

            // Skip boundaries before or at chunk_start
            if offset <= chunk_start {
                boundary_idx += 1;
                continue;
            }

            let chunk_size = offset - chunk_start;

            // Safety valve: if we've accumulated >= MAX without finding a
            // boundary, we need to force-split. Break out and handle below.
            if chunk_size >= CHUNK_MAX {
                break;
            }

            if chunk_size < CHUNK_MIN {
                // Too small — skip this boundary
                boundary_idx += 1;
                continue;
            }

            if chunk_size < CHUNK_TARGET {
                // Between MIN and TARGET — only split on high-score boundaries
                if score >= 3 {
                    split_at = Some(offset);
                    boundary_idx += 1;
                    break;
                }
                boundary_idx += 1;
                continue;
            }

            // chunk_size >= TARGET (and < MAX since we checked above)
            if score >= 1 {
                split_at = Some(offset);
                boundary_idx += 1;
                break;
            }
            boundary_idx += 1;
        }

        if let Some(offset) = split_at {
            chunks.push(content[chunk_start..offset].to_vec());
            chunk_start = offset;
        } else {
            // No suitable boundary found — check if we need a force split
            let remaining = content.len() - chunk_start;

            if remaining > CHUNK_MAX {
                // Check if there's ANY boundary in the next MAX window
                let max_end = chunk_start + CHUNK_MAX;
                let mut found_boundary = false;

                // Re-scan boundaries from current index for force-split region
                let mut scan_idx = boundary_idx;
                while scan_idx < boundaries.len() {
                    let (offset, _score) = boundaries[scan_idx];
                    if offset <= chunk_start {
                        scan_idx += 1;
                        continue;
                    }
                    if offset > max_end {
                        break;
                    }
                    let chunk_size = offset - chunk_start;
                    if chunk_size >= CHUNK_MIN {
                        // Use this boundary even though it didn't meet the
                        // score threshold earlier (we're in force-split territory).
                        chunks.push(content[chunk_start..offset].to_vec());
                        chunk_start = offset;
                        boundary_idx = scan_idx + 1;
                        found_boundary = true;
                        break;
                    }
                    scan_idx += 1;
                }

                if !found_boundary {
                    // No boundary at all — use FastCDC on this segment
                    let segment_end = content.len().min(chunk_start + CHUNK_MAX);
                    let segment = &content[chunk_start..segment_end];

                    // If the segment is large enough for FastCDC
                    if segment.len() > CHUNK_MIN {
                        let sub_chunks = fastcdc_chunk(segment);
                        let total_consumed: usize =
                            sub_chunks.iter().map(|c| c.len()).sum();
                        chunks.extend(sub_chunks);
                        chunk_start += total_consumed;
                    } else {
                        // Just emit what we have
                        chunks.push(segment.to_vec());
                        chunk_start = segment_end;
                    }
                }
            } else {
                // Remaining content fits in one chunk — emit it
                chunks.push(content[chunk_start..].to_vec());
                break;
            }
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: generate a Rust source file with multiple functions, each
    /// padded to roughly `body_lines` lines. Body uses `println!` so the
    /// lines do NOT match any top-level declaration keyword.
    fn rust_source(fn_count: usize, body_lines: usize) -> String {
        let mut src = String::new();
        for i in 0..fn_count {
            if i > 0 {
                src.push('\n');
            }
            src.push_str(&format!("pub fn function_{}() {{\n", i));
            for j in 0..body_lines {
                src.push_str(&format!(
                    "        println!(\"value_{}_{{}}\", {});\n",
                    j,
                    j * i + 1
                ));
            }
            src.push_str("}\n");
        }
        src
    }

    /// Helper: generate Python source with multiple functions. Body uses
    /// `print()` so lines do NOT match declaration keywords.
    fn python_source(fn_count: usize, body_lines: usize) -> String {
        let mut src = String::new();
        for i in 0..fn_count {
            if i > 0 {
                src.push('\n');
                src.push('\n');
            }
            src.push_str(&format!("def function_{}():\n", i));
            for j in 0..body_lines {
                src.push_str(&format!(
                    "        print(\"value_{}_{{}}\".format({}))\n",
                    j,
                    j * i + 1
                ));
            }
        }
        src
    }

    #[test]
    fn rust_source_splits_at_function_boundaries() {
        // Each function: ~40 lines * ~25 bytes/line ≈ 1000 bytes
        // 6 functions ≈ 6000 bytes, should produce multiple chunks split at fn boundaries
        let src = rust_source(6, 40);
        let chunks = chunk(src.as_bytes());

        assert!(chunks.len() > 1, "expected multiple chunks, got {}", chunks.len());

        // Verify total content is preserved
        let reassembled: Vec<u8> = chunks.iter().flatten().copied().collect();
        assert_eq!(reassembled, src.as_bytes());

        // Each chunk should respect MIN/MAX
        for (i, c) in chunks.iter().enumerate() {
            assert!(
                c.len() <= CHUNK_MAX,
                "chunk {} exceeds MAX: {} bytes",
                i,
                c.len()
            );
        }

        // Chunks split at scored boundaries near function edges. Because `}`
        // at indent 0 scores 3, splits land just before the closing brace,
        // so each chunk (after the first) starts with `}\n\npub fn ...`.
        // Verify that most chunks contain exactly one `pub fn` signature
        // (allowing for the boundary chunk that may straddle two).
        let mut chunks_with_fn = 0;
        for c in &chunks {
            let text = String::from_utf8_lossy(c);
            if text.contains("pub fn function_") {
                chunks_with_fn += 1;
            }
        }
        assert!(
            chunks_with_fn >= 5,
            "expected most chunks to contain a function, got {} of {}",
            chunks_with_fn,
            chunks.len()
        );
    }

    #[test]
    fn python_source_splits_at_def_class() {
        // Each function: ~50 lines * ~20 bytes/line ≈ 1000 bytes
        let mut src = python_source(6, 50);
        // Add a class too
        src.push_str("\n\nclass MyClass:\n");
        for j in 0..50 {
            src.push_str(&format!("    attr_{} = {}\n", j, j));
        }

        let chunks = chunk(src.as_bytes());
        assert!(chunks.len() > 1, "expected multiple chunks, got {}", chunks.len());

        let reassembled: Vec<u8> = chunks.iter().flatten().copied().collect();
        assert_eq!(reassembled, src.as_bytes());

        // At least one chunk (after the first) should start at a def/class boundary
        let mut boundary_count = 0;
        for c in &chunks[1..] {
            let text = String::from_utf8_lossy(c);
            let trimmed = text.trim_start_matches('\n');
            if trimmed.starts_with("def ") || trimmed.starts_with("class ") {
                boundary_count += 1;
            }
        }
        assert!(
            boundary_count > 0,
            "expected at least one chunk starting at def/class"
        );
    }

    #[test]
    fn single_function_edit_stability() {
        // Create a file with 5 functions, each large enough for its own chunk.
        // Body uses `println!` at 8-space indent so lines don't match keywords.
        let mut functions: Vec<String> = Vec::new();
        for i in 0..5 {
            let mut f = format!("pub fn function_{}() {{\n", i);
            for j in 0..60 {
                f.push_str(&format!(
                    "        println!(\"val_{}_{{}}\", {});\n",
                    j,
                    j + i * 100
                ));
            }
            f.push_str("}\n\n");
            functions.push(f);
        }

        let original = functions.join("");
        let original_chunks = chunk(original.as_bytes());
        let original_hashes: Vec<blake3::Hash> = original_chunks
            .iter()
            .map(|c| blake3::hash(c))
            .collect();

        // Modify only function_2's body
        let mut modified_functions = functions.clone();
        let mut f = format!("pub fn function_2() {{\n");
        for j in 0..60 {
            f.push_str(&format!(
                "        println!(\"modified_{}_{{}}\", {});\n",
                j,
                j + 9999
            ));
        }
        f.push_str("}\n\n");
        modified_functions[2] = f;

        let modified = modified_functions.join("");
        let modified_chunks = chunk(modified.as_bytes());
        let modified_hashes: Vec<blake3::Hash> = modified_chunks
            .iter()
            .map(|c| blake3::hash(c))
            .collect();

        // The number of chunks should be the same (structural stability)
        assert_eq!(
            original_chunks.len(),
            modified_chunks.len(),
            "chunk count changed: {} -> {}",
            original_chunks.len(),
            modified_chunks.len()
        );

        // Count unchanged chunks — most should have identical hashes
        let unchanged = original_hashes
            .iter()
            .zip(modified_hashes.iter())
            .filter(|(a, b)| a == b)
            .count();

        assert!(
            unchanged >= original_hashes.len() - 2,
            "too many chunks changed: only {} of {} unchanged",
            unchanged,
            original_hashes.len()
        );

        // At least one chunk should have changed (the one containing function_2)
        assert!(
            unchanged < original_hashes.len(),
            "expected at least one chunk to change"
        );
    }

    #[test]
    fn small_file_single_chunk() {
        let content = b"fn main() {\n    println!(\"hello\");\n}\n";
        assert!(content.len() < CHUNK_MIN);

        let chunks = chunk(content);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], content);
    }

    #[test]
    fn large_file_no_boundaries_fastcdc_fallback() {
        // Create a large file with no structural boundaries — one long repeated
        // line with no empty lines, no keywords, etc.
        let line = "abcdefghijklmnopqrstuvwxyz0123456789____";
        let mut content = String::new();
        while content.len() < CHUNK_MAX * 3 {
            content.push_str(line);
            content.push('\n');
        }

        let chunks = chunk(content.as_bytes());
        assert!(chunks.len() > 1, "expected multiple chunks from fastcdc fallback");

        // Every chunk must respect MAX
        for (i, c) in chunks.iter().enumerate() {
            assert!(
                c.len() <= CHUNK_MAX,
                "chunk {} exceeds MAX: {} bytes",
                i,
                c.len()
            );
        }

        // Content must be fully preserved
        let reassembled: Vec<u8> = chunks.iter().flatten().copied().collect();
        assert_eq!(reassembled, content.as_bytes());
    }

    #[test]
    fn binary_file_uses_fastcdc() {
        // Create binary content with null bytes
        let mut content = vec![0u8; CHUNK_MAX * 2];
        for (i, b) in content.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        // Ensure there's a null byte in the first 8192 bytes
        content[100] = 0x00;

        assert!(is_binary(&content));

        let chunks = chunk(&content);
        assert!(!chunks.is_empty());

        // Content preserved
        let reassembled: Vec<u8> = chunks.iter().flatten().copied().collect();
        assert_eq!(reassembled, content);

        // MAX respected
        for (i, c) in chunks.iter().enumerate() {
            assert!(
                c.len() <= CHUNK_MAX,
                "chunk {} exceeds MAX: {} bytes",
                i,
                c.len()
            );
        }
    }

    #[test]
    fn empty_content() {
        let chunks = chunk(b"");
        assert!(chunks.is_empty());
    }

    #[test]
    fn exact_max_boundary_no_boundaries() {
        // Content that's exactly MAX bytes with no structural boundaries
        // One continuous block of 'a' characters with newlines spaced far apart
        let mut content = vec![b'a'; CHUNK_MAX];
        // Put a newline near the end just to make it "text" — but no
        // structural boundaries
        content[CHUNK_MAX - 1] = b'\n';

        assert!(!is_binary(&content));

        let chunks = chunk(&content);
        // Should be a single chunk since it's exactly MAX
        assert_eq!(chunks.len(), 1, "expected 1 chunk, got {}", chunks.len());
        assert_eq!(chunks[0].len(), CHUNK_MAX);
    }

    #[test]
    fn score_boundaries_rust_keywords() {
        let src = b"use std::io;\n\npub fn main() {\n    let x = 1;\n}\n";
        let boundaries = score_boundaries(src);

        // There should be boundaries scored
        assert!(!boundaries.is_empty());

        // Find the boundary before `pub fn main()`
        let pub_fn_offset = src
            .windows(6)
            .position(|w| w == b"pub fn")
            .unwrap();
        let has_fn_boundary = boundaries
            .iter()
            .any(|(off, score)| *off == pub_fn_offset && *score >= 4);
        assert!(
            has_fn_boundary,
            "expected score >= 4 at pub fn boundary (offset {}), boundaries: {:?}",
            pub_fn_offset, boundaries
        );
    }

    #[test]
    fn score_boundaries_import_edge() {
        let src = b"import os\nimport sys\n\ndef main():\n    pass\n";
        let boundaries = score_boundaries(src);

        // Boundary between "import os" and "import sys" should NOT be an
        // import edge (both are imports). But boundary at the transition
        // from import to non-import should score.
        assert!(!boundaries.is_empty());
    }

    #[test]
    fn content_fully_preserved() {
        // Verify that for various inputs, concatenating all chunks reproduces
        // the original content exactly.
        let inputs: Vec<&[u8]> = vec![
            b"hello world\n",
            rust_source(3, 20).as_bytes().to_vec().leak(),
            python_source(4, 30).as_bytes().to_vec().leak(),
        ];

        for input in inputs {
            let chunks = chunk(input);
            let reassembled: Vec<u8> = chunks.iter().flatten().copied().collect();
            assert_eq!(
                reassembled, input,
                "content not preserved for input of {} bytes",
                input.len()
            );
        }
    }
}
