//! On-disk MCP response cache (§6): `cache/mcp/<server>/<tool>/<argsHash>.jsonl`
//! plus a `.meta.json` sidecar. Atomic writes (tmp + rename); entries whose meta
//! is missing, expired, or `complete: false` are treated as misses.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMeta {
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
    #[serde(rename = "ttlS")]
    pub ttl_s: u64,
    pub complete: bool,
    pub pages: u32,
    #[serde(rename = "totalItems", skip_serializing_if = "Option::is_none")]
    pub total_items: Option<u64>,
    #[serde(rename = "sourceArgs")]
    pub source_args: Value,
}

impl CacheMeta {
    pub fn is_fresh(&self, now: u64) -> bool {
        now < self.fetched_at.saturating_add(self.ttl_s)
    }
}

/// Internal page index (`<hash>.pages.json`) — records which result field holds
/// the item array and how many items each page contributed, so later paginated
/// calls can slice the JSONL file. Not part of the pinned meta format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageIndex {
    pub field: String,
    pub counts: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub meta: CacheMeta,
    pub hash: String,
}

#[derive(Clone)]
pub struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Normalized-args hash (§6): recursively sorted object keys, compact JSON
    /// (no whitespace variance), sha256 hex. `key_fields` (from `cache.key`)
    /// restricts the hash to those args; `exclude` drops an arg (the pagination
    /// param) so all pages of one query share a hash.
    pub fn hash_args(args: &Value, key_fields: Option<&[String]>, exclude: Option<&str>) -> String {
        let selected = match (key_fields, args) {
            (Some(keys), Value::Object(map)) if !keys.is_empty() => {
                let mut subset = serde_json::Map::new();
                for k in keys {
                    if let Some(v) = map.get(k) {
                        subset.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(subset)
            }
            _ => args.clone(),
        };
        let selected = match (exclude, selected) {
            (Some(param), Value::Object(mut map)) => {
                map.remove(param);
                Value::Object(map)
            }
            (_, v) => v,
        };
        let mut normalized = String::new();
        write_normalized(&selected, &mut normalized);
        let digest = Sha256::digest(normalized.as_bytes());
        hex_encode(&digest)
    }

    fn dir(&self, server: &str, tool: &str) -> PathBuf {
        self.root
            .join(safe_path_component(server))
            .join(safe_path_component(tool))
    }

    fn jsonl_path(&self, server: &str, tool: &str, hash: &str) -> Result<PathBuf> {
        validate_hash(hash)?;
        Ok(self.dir(server, tool).join(format!("{hash}.jsonl")))
    }

    fn meta_path(&self, server: &str, tool: &str, hash: &str) -> Result<PathBuf> {
        validate_hash(hash)?;
        Ok(self.dir(server, tool).join(format!("{hash}.meta.json")))
    }

    fn pages_path(&self, server: &str, tool: &str, hash: &str) -> Result<PathBuf> {
        validate_hash(hash)?;
        Ok(self.dir(server, tool).join(format!("{hash}.pages.json")))
    }

    /// Returns the entry iff the meta exists, is complete, and is within TTL.
    /// `complete: false` (in-progress or crashed fetch) is treated as a miss (§6).
    pub fn fresh_entry(&self, server: &str, tool: &str, hash: &str) -> Result<Option<CacheEntry>> {
        Ok(self
            .complete_entry(server, tool, hash)?
            .filter(|entry| entry.meta.is_fresh(now_secs())))
    }

    /// A complete owned generation, regardless of TTL. Used to publish the
    /// just-fetched worklist even when the caller requested a zero cache TTL.
    pub fn complete_entry(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
    ) -> Result<Option<CacheEntry>> {
        let meta_path = self.meta_path(server, tool, hash)?;
        if !meta_path.exists() || !self.jsonl_path(server, tool, hash)?.exists() {
            return Ok(None);
        }
        let meta: CacheMeta =
            serde_json::from_str(&fs::read_to_string(&meta_path).context("reading cache meta")?)
                .context("parsing cache meta")?;
        if !meta.complete {
            return Ok(None);
        }
        Ok(Some(CacheEntry {
            meta,
            hash: hash.to_string(),
        }))
    }

    /// All complete + fresh entries for a tool (used by serveFrom lookup).
    pub fn fresh_entries_for_tool(&self, server: &str, tool: &str) -> Result<Vec<CacheEntry>> {
        let dir = self.dir(server, tool);
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(hash) = name.strip_suffix(".meta.json") {
                if validate_hash(hash).is_err() {
                    continue;
                }
                if let Some(e) = self.fresh_entry(server, tool, hash)? {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    /// Reads all JSONL lines of an entry as JSON values.
    pub fn read_items(&self, server: &str, tool: &str, hash: &str) -> Result<Vec<Value>> {
        let path = self.jsonl_path(server, tool, hash)?;
        let file = fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let mut items = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            items.push(serde_json::from_str(&line).context("parsing cached JSONL line")?);
        }
        Ok(items)
    }

    /// Slices a complete paginated entry: `page` is 1-based. Returns the items
    /// and whether a further page exists.
    pub fn read_page(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        page: u64,
    ) -> Result<Option<(Vec<Value>, PageIndex, bool)>> {
        Ok(self
            .read_page_with_size(server, tool, hash, page)?
            .map(|(items, index, has_next, _)| (items, index, has_next)))
    }

    /// Validate the complete JSONL generation while retaining only the requested
    /// page. Count compact array bytes in this same scan, avoiding a second
    /// collection read or allocation solely for response telemetry.
    #[allow(clippy::type_complexity)]
    pub(crate) fn read_page_with_size(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        page: u64,
    ) -> Result<Option<(Vec<Value>, PageIndex, bool, u64)>> {
        let index = self.read_page_index(server, tool, hash)?;
        let Some(index) = index else { return Ok(None) };
        if page == 0 || page > index.counts.len() as u64 {
            return Ok(None);
        }
        let sum = |counts: &[u64]| -> Result<u64> {
            counts.iter().try_fold(0u64, |sum, count| {
                sum.checked_add(*count)
                    .context("cached page item count overflow")
            })
        };
        let start = sum(&index.counts[..(page - 1) as usize])?;
        let end = start
            .checked_add(index.counts[(page - 1) as usize])
            .context("cached page item count overflow")?;
        let expected = sum(&index.counts)?;
        let path = self.jsonl_path(server, tool, hash)?;
        let file = fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let mut selected = Vec::new();
        let mut count = 0u64;
        let mut bytes = 2u64; // Opening and closing array brackets, including [].
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line).context("parsing cached JSONL line")?;
            let size = crate::json_size::compact_bytes(&value)?;
            bytes = bytes
                .checked_add(size)
                .and_then(|bytes| bytes.checked_add(u64::from(count != 0)))
                .context("compact collection byte count overflow")?;
            if count >= start && count < end {
                selected.push(value);
            }
            count = count.checked_add(1).context("cached item count overflow")?;
        }
        ensure!(
            count == expected,
            "cached collection item count does not match page index"
        );
        let has_next = page < index.counts.len() as u64;
        Ok(Some((selected, index, has_next, bytes)))
    }

    fn read_page_index(&self, server: &str, tool: &str, hash: &str) -> Result<Option<PageIndex>> {
        let path = self.pages_path(server, tool, hash)?;
        if !path.exists() {
            return Ok(None);
        }
        let index =
            serde_json::from_str(&fs::read_to_string(&path)?).context("parsing page index")?;
        Ok(Some(index))
    }

    /// Writes a complete single-result cache entry (one JSONL line) atomically.
    pub fn write_single(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        result: &Value,
        ttl_s: u64,
        source_args: &Value,
    ) -> Result<()> {
        let meta = CacheMeta {
            fetched_at: now_secs(),
            ttl_s,
            complete: true,
            pages: 1,
            total_items: Some(1),
            source_args: source_args.clone(),
        };
        self.write_entry(server, tool, hash, std::slice::from_ref(result), &meta)
    }

    /// Starts an in-progress paginated entry: page-1 items on disk,
    /// `complete: false` meta (readers treat it as a miss until finalize).
    // Keep the explicit cache identity/page metadata API during prototype bring-up.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_paginated(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        field: &str,
        page1: &[Value],
        ttl_s: u64,
        source_args: &Value,
    ) -> Result<()> {
        validate_hash(hash)?;
        let dir = self.dir(server, tool);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.write_items_atomic(server, tool, hash, page1)?;
        let index = PageIndex {
            field: field.to_string(),
            counts: vec![page1.len() as u64],
        };
        self.write_json_atomic(&self.pages_path(server, tool, hash)?, &index)?;
        let meta = CacheMeta {
            fetched_at: now_secs(),
            ttl_s,
            complete: false,
            pages: 1,
            total_items: None,
            source_args: source_args.clone(),
        };
        self.write_json_atomic(&self.meta_path(server, tool, hash)?, &meta)
    }

    /// Appends one fetched page to an in-progress entry and updates the index.
    pub fn append_page(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        page_items: &[Value],
    ) -> Result<u32> {
        let path = self.jsonl_path(server, tool, hash)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .with_context(|| format!("appending {}", path.display()))?;
        for item in page_items {
            serde_json::to_writer(&mut file, item)?;
            file.write_all(b"\n")?;
        }
        let mut index = self
            .read_page_index(server, tool, hash)?
            .context("page index missing for in-progress entry")?;
        index.counts.push(page_items.len() as u64);
        self.write_json_atomic(&self.pages_path(server, tool, hash)?, &index)?;
        Ok(index.counts.len() as u32)
    }

    /// Flips the meta to `complete: true` (atomic) at the end of a background fetch.
    pub fn finalize_paginated(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        ttl_s: u64,
        source_args: &Value,
    ) -> Result<CacheMeta> {
        let index = self
            .read_page_index(server, tool, hash)?
            .context("page index missing at finalize")?;
        let total: u64 = index.counts.iter().sum();
        let meta = CacheMeta {
            fetched_at: now_secs(),
            ttl_s,
            complete: true,
            pages: index.counts.len() as u32,
            total_items: Some(total),
            source_args: source_args.clone(),
        };
        self.write_json_atomic(&self.meta_path(server, tool, hash)?, &meta)?;
        Ok(meta)
    }

    fn write_entry(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        items: &[Value],
        meta: &CacheMeta,
    ) -> Result<()> {
        validate_hash(hash)?;
        let dir = self.dir(server, tool);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.write_items_atomic(server, tool, hash, items)?;
        self.write_json_atomic(&self.meta_path(server, tool, hash)?, meta)
    }

    fn write_items_atomic(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
        items: &[Value],
    ) -> Result<()> {
        let path = self.jsonl_path(server, tool, hash)?;
        let tmp = path.with_extension("jsonl.tmp");
        {
            let mut file =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            for item in items {
                serde_json::to_writer(&mut file, item)?;
                file.write_all(b"\n")?;
            }
            file.sync_all().ok();
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))
    }

    fn write_json_atomic<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut file =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            serde_json::to_writer(&mut file, value)?;
            file.write_all(b"\n")?;
            file.sync_all().ok();
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))
    }
}

/// Writes `value` to `path` atomically (tmp + rename). Returns bytes written.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(bytes.len() as u64)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Canonical JSON with recursively sorted object keys and no whitespace.
pub fn normalized_json(value: &Value) -> String {
    let mut out = String::new();
    write_normalized(value, &mut out);
    out
}

fn write_normalized(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("string serialization is infallible"))
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_normalized(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(
                    &serde_json::to_string(key).expect("string serialization is infallible"),
                );
                out.push(':');
                write_normalized(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn validate_hash(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "cache hash must be 64 lowercase hexadecimal characters"
    );
    Ok(())
}

/// Storage identifiers only: preserve common names, hash every other name into
/// one bounded component. The reserved `~` prefix separates encoded identifiers
/// from preserved names. Never fall back to reading an unsafe legacy path.
pub(crate) fn safe_path_component(name: &str) -> String {
    if !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        name.to_string()
    } else {
        format!("~{}", hex_encode(&Sha256::digest(name.as_bytes())))
    }
}

#[cfg(test)]
mod path_tests {
    use super::safe_path_component;

    #[test]
    fn encoded_components_are_bounded_and_disjoint_from_preserved_names() {
        assert_eq!(safe_path_component("list_issues-2"), "list_issues-2");
        for name in [
            "",
            ".",
            "..",
            "/absolute",
            "../relative",
            "a/b",
            "a\\b",
            "café",
            &"a".repeat(81),
        ] {
            let encoded = safe_path_component(name);
            assert_eq!(encoded.len(), 65);
            assert!(encoded.starts_with('~'));
            assert!(encoded[1..].bytes().all(|b| b.is_ascii_hexdigit()));
            assert_eq!(safe_path_component(name), encoded);
            assert_ne!(safe_path_component(&encoded), encoded);
        }
        assert_ne!(safe_path_component("a/b"), safe_path_component("a\\b"));
    }
}
