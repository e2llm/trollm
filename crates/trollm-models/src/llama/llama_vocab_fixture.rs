//! Self-authored GGUF v3 metadata for native grammar tests; no model weights.
//! Token names enumerate bytes rather than copying a pretrained vocabulary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct VocabularyFixture {
    directory: PathBuf,
    path: PathBuf,
}

impl VocabularyFixture {
    pub(super) fn new() -> io::Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let directory = loop {
            let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir().join(format!(
                "trollm-grammar-vocabulary-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let fixture = Self {
            path: directory.join("vocabulary.gguf"),
            directory,
        };
        fs::write(&fixture.path, vocabulary_gguf())?;
        Ok(fixture)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for VocabularyFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

// GGUF v3 uses little-endian integers, u64 string/array lengths, and typed
// metadata. Values match ggml/include/gguf.h in pinned llama-cpp-sys-2 0.1.159.
const UINT32: u32 = 4;
const INT32: u32 = 5;
const FLOAT32: u32 = 6;
const BOOL: u32 = 7;
const STRING: u32 = 8;
const ARRAY: u32 = 9;

fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn key(bytes: &mut Vec<u8>, name: &str, kind: u32) {
    string(bytes, name);
    bytes.extend_from_slice(&kind.to_le_bytes());
}

fn array(bytes: &mut Vec<u8>, name: &str, element_kind: u32, len: usize) {
    key(bytes, name, ARRAY);
    bytes.extend_from_slice(&element_kind.to_le_bytes());
    bytes.extend_from_slice(&(len as u64).to_le_bytes());
}

fn vocabulary_gguf() -> Vec<u8> {
    let mut tokens: Vec<String> = ["<unk>", "<s>", "</s>", "▁"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    tokens.extend((0..=255).map(|byte| format!("<0x{byte:02X}>")));

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes()); // no tensors
    bytes.extend_from_slice(&12u64.to_le_bytes()); // metadata entries below

    key(&mut bytes, "general.architecture", STRING);
    string(&mut bytes, "llama");
    key(&mut bytes, "tokenizer.ggml.model", STRING);
    string(&mut bytes, "llama");
    array(&mut bytes, "tokenizer.ggml.tokens", STRING, tokens.len());
    for token in &tokens {
        string(&mut bytes, token);
    }
    array(&mut bytes, "tokenizer.ggml.scores", FLOAT32, tokens.len());
    for _ in &tokens {
        bytes.extend_from_slice(&0f32.to_le_bytes());
    }
    array(&mut bytes, "tokenizer.ggml.token_type", INT32, tokens.len());
    // UNKNOWN, CONTROL, CONTROL, NORMAL, then BYTE (llama.h token types).
    for kind in [2i32, 3, 3, 1]
        .into_iter()
        .chain(std::iter::repeat_n(6, 256))
    {
        bytes.extend_from_slice(&kind.to_le_bytes());
    }
    for (name, id) in [
        ("tokenizer.ggml.unknown_token_id", 0u32),
        ("tokenizer.ggml.bos_token_id", 1),
        ("tokenizer.ggml.eos_token_id", 2),
    ] {
        key(&mut bytes, name, UINT32);
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    for name in [
        "tokenizer.ggml.add_space_prefix",
        "tokenizer.ggml.remove_extra_whitespaces",
        "tokenizer.ggml.add_bos_token",
        "tokenizer.ggml.add_eos_token",
    ] {
        key(&mut bytes, name, BOOL);
        bytes.push(0);
    }
    // Default GGUF alignment, even though the tensor-data section is empty.
    bytes.resize(bytes.len().next_multiple_of(32), 0);
    bytes
}
