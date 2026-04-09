// Storage is a library module — many methods are used by tests and compaction
// but not directly from the CLI binary.
#![allow(dead_code)]

pub mod chunker;
pub mod codec;
pub mod compaction;
pub mod engine;
pub mod index;
pub mod log;
pub mod pipeline;
