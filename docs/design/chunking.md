# Structural Chunking

Seven doesn't chunk files at arbitrary byte boundaries. It splits at **structural boundaries** — the natural seams in code — so that edits to one function don't invalidate chunks belonging to the next.

No parser. No AST. Just lightweight line-scanning heuristics that work across languages.

## The Problem with Naive Chunking

FastCDC uses a rolling hash over raw bytes to find split points. This works well for binary data, but code has structure that the rolling hash ignores:

```
fn authorize(token: &str) -> Result<User> {  ← chunk boundary lands here
    let claims = decode(token)?;               by coincidence
    validate_expiry(&claims)?;
    lookup_user(claims.sub)
}
                                              ← this blank line is the real boundary
fn refresh(token: &str) -> Result<Token> {
    ...
}
```

When an agent edits `authorize`, the rolling hash shifts. The chunk that happened to start mid-function now starts somewhere else. Chunks that contain `refresh` — which didn't change — get a different hash. Dedup breaks.

## Approach: Boundary-Biased Chunking

Two passes over the file. First pass finds structural boundaries. Second pass uses those boundaries to guide where chunks actually split.

### Pass 1: Boundary Detection

Scan the file line by line. Score each line as a potential split point:

| Signal | Score | Example |
|--------|-------|---------|
| Empty line | 3 | `\n\n` — universal block separator |
| Double empty line | 5 | `\n\n\n` — section separator |
| Top-level declaration | 4 | `fn `, `def `, `class `, `struct `, `impl `, `export `, `module ` at indent 0 |
| Decorator / attribute at indent 0 | 4 | `@`, `#[` — usually precedes a declaration |
| Closing brace at indent 0 | 3 | `}` alone on a line at column 0 |
| Comment block boundary | 2 | `//`, `#`, `/*`, `"""`, `'''` after a non-comment line |
| Indentation drop to 0 | 2 | Line goes from indented back to column 0 |
| `import` / `use` / `require` block edge | 2 | Transition from imports to non-imports or vice versa |

The scanner doesn't need to know the language. These patterns overlap heavily across languages:

- **Rust**: `fn`, `struct`, `enum`, `impl`, `mod`, `pub fn`, `#[`
- **Python**: `def`, `class`, `@`, blank lines (PEP 8 requires them)
- **TypeScript/JavaScript**: `function`, `class`, `export`, `const X = `, blank lines
- **Go**: `func`, `type`, blank lines
- **Swift**: `func`, `struct`, `class`, `enum`, `@`, blank lines
- **C/C++**: `void`, `int`, `class`, `struct`, `#include`, `}` at indent 0

Every language uses blank lines between logical blocks. That alone covers most cases.

### Pass 2: Boundary-Guided Splitting

With scored boundaries in hand, split the file into chunks:

```
split(file, boundaries, min=512B, target=4KB, max=16KB) -> chunks[]
```

1. Walk the file sequentially, accumulating bytes into the current chunk
2. At each boundary, check the current chunk size:
   - **Below `min`**: skip this boundary, keep accumulating (don't produce tiny chunks)
   - **Between `min` and `target`**: split here if the boundary score is >= 3 (strong boundary)
   - **Between `target` and `max`**: split here at any boundary score >= 1
   - **At `max`**: force-split, even mid-line if necessary (safety valve)
3. If no boundary is found before `max`, fall back to FastCDC's rolling hash for that segment

This means:
- Small files (< `min`) are a single chunk
- Well-structured code splits at blank lines and declarations
- Dense code without clear boundaries falls back to content-defined splitting
- No chunk ever exceeds `max`

### Example

Input file (200 lines, ~6KB):

```
use std::io;                     ─┐
use std::path::Path;              │ imports block
                          ← score 3 (empty line)
/// Configuration loader  ─┐
struct Config {             │
    path: PathBuf,          │ ~1.2KB section
    values: HashMap,        │
}                           │
                          ← score 5 (double empty + indent drop)

impl Config {             ─┐
    fn load() -> Self {     │
        ...                 │ ~2.8KB section
    }                       │
                          ← score 3 (empty line)
    fn get(&self) -> &str { │
        ...                 │ ~1.5KB section
    }                       │
}                           │
                          ← score 5 (closing brace at indent 0 + empty line)

fn main() {               ─┐
    ...                     │ ~0.8KB section
}                          ─┘
```

Result: 4 chunks aligned to logical boundaries.

With naive FastCDC, this same file might produce 1-2 chunks split mid-function. Editing `Config::load` would invalidate a chunk that also contains part of `Config::get`.

## Size Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `min` | 512 bytes | Avoid fragment chunks for one-liner functions |
| `target` | 4 KB | Sweet spot for code — roughly 80–120 lines |
| `max` | 16 KB | Safety cap, ~400 lines — force split if no boundary found |

These are defaults. The server can tune them per-repo in `config.toml`:

```toml
[chunking]
min_bytes = 512
target_bytes = 4096
max_bytes = 16384
```

## Binary and Non-Code Files

The structural scanner only works on UTF-8 text. For binary files or files that fail UTF-8 validation:

- Skip pass 1 entirely
- Use pure FastCDC (rolling hash, no boundary hints)
- Same min/target/max parameters

Detection: attempt UTF-8 validation on the first 8KB. If it fails, or if the file extension is in a known binary set (`.png`, `.jpg`, `.wasm`, `.zip`, etc.), treat as binary.

## Dedup Characteristics

Structural chunking improves dedup for code because chunks align to **semantic units** rather than arbitrary byte offsets:

- **Single function edit**: only the chunk(s) containing that function change. Adjacent functions keep their hashes.
- **Added function**: a new chunk appears. Surrounding chunks stay the same if the blank-line boundaries are preserved.
- **Reordered functions**: chunks move in the blob's chunk list, but each chunk's hash is unchanged. The snapshot changes (different blob), but no new chunk data is stored.
- **Moved file**: identical chunks, different path in the snapshot. Zero new storage.

Compared to naive FastCDC on code, structural chunking reduces chunk churn by aligning splits to the boundaries that humans and agents naturally edit around.

## What This Is Not

- **Not a parser.** No grammar, no token stream, no syntax tree. A file with broken syntax chunks just fine — the scanner just sees lines and indentation.
- **Not language-specific.** The same scanner handles Rust, Python, TypeScript, Go, and anything else with blank lines and indentation. No per-language configuration.
- **Not line-level tracking.** Chunks still contain raw bytes. The line scanning is only used to decide *where* to split. After splitting, chunks are opaque byte sequences hashed and stored like any other.
