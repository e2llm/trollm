//! Artifact store (§8): `runs/<uuid>/artifacts/`, atomic writes (tmp + rename),
//! sha256 recorded for `artifact_written` events (§7). Meta sidecars
//! (`<name>.meta.json` with `complete: bool`, §6) are written by the producing
//! side (gateway for MCP fetches, core for foreach snapshots) and read by the
//! verifier (§4.3).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::plan::hex_sha256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    #[serde(rename = "fetchedAt", default)]
    pub fetched_at: u64,
    #[serde(default)]
    pub complete: bool,
}

/// A recorded artifact write: what `artifact_written` events carry.
#[derive(Debug, Clone)]
pub struct WrittenArtifact {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Original foreach JSONL and content address, shared by writer and consumers.
pub fn foreach_snapshot_bytes(items: &[serde_json::Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for item in items {
        bytes.extend_from_slice(item.to_string().as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

pub fn foreach_snapshot_name(node: &str, bytes: &[u8]) -> String {
    format!("{node}.{}.snapshot.jsonl", hex_sha256(bytes))
}

pub fn write_foreach_snapshot(
    directory: &Path,
    node: &str,
    items: &[serde_json::Value],
) -> std::io::Result<WrittenArtifact> {
    let bytes = foreach_snapshot_bytes(items);
    write_atomic(directory, &foreach_snapshot_name(node, &bytes), &bytes)
}

/// Atomic write (tmp + rename) of `bytes` to `dir/<name>`; returns path, size
/// and sha256 for the event log.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<WrittenArtifact> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(WrittenArtifact {
        path,
        bytes: bytes.len() as u64,
        sha256: hex_sha256(bytes),
    })
}

/// Private diagnostic write: restrict access from creation on Unix hosts.
/// Other hosts rely on the enclosing app-private run storage.
pub(crate) fn write_private_atomic(
    dir: &Path,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<WrittenArtifact> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    let result = file.write_all(bytes).and_then(|()| {
        drop(file);
        std::fs::rename(&tmp, &path)
    });
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(WrittenArtifact {
        path,
        bytes: bytes.len() as u64,
        sha256: hex_sha256(bytes),
    })
}

/// Write the `<file>.meta.json` sidecar next to an artifact.
pub fn write_meta(path: &Path, meta: &ArtifactMeta) -> std::io::Result<()> {
    let sidecar = meta_path(path);
    let bytes = serde_json::to_vec_pretty(meta).unwrap_or_else(|_| b"{}".to_vec());
    let tmp = sidecar.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &sidecar)?;
    Ok(())
}

pub fn meta_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".meta.json");
    PathBuf::from(p)
}

/// §4.3 artifact completeness: the meta sidecar must exist and say
/// `complete: true`. Missing/unparseable sidecar = incomplete.
pub fn is_complete(path: &Path) -> bool {
    std::fs::read(meta_path(path))
        .ok()
        .and_then(|b| serde_json::from_slice::<ArtifactMeta>(&b).ok())
        .map(|m| m.complete)
        .unwrap_or(false)
}

/// Logical artifact name convention (wiring note): the gateway/core name
/// artifact files `<logicalName>.<ext>`; the logical name used in
/// `fromArtifact` / `requireArtifactComplete` (e.g. `tracker.list_issues.latest`)
/// is the filename with its final extension stripped.
pub fn logical_name(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn actual_foreach_writer_preserves_zero_one_many_and_distinct_revisions() {
        let root = std::env::temp_dir().join(format!("foreach-writer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for items in [
            vec![],
            vec![json!({"id":"one","value":"é"})],
            vec![
                json!({"id":"one"}),
                json!({"id":"two"}),
                json!({"id":"three"}),
            ],
        ] {
            let written = write_foreach_snapshot(&root, "items", &items).unwrap();
            let raw = std::fs::read(&written.path).unwrap();
            assert_eq!(written.sha256, hex_sha256(&raw));
            assert_eq!(written.bytes, raw.len() as u64);
            assert_eq!(
                written.path.file_name().unwrap().to_str().unwrap(),
                format!("items.{}.snapshot.jsonl", written.sha256)
            );
            let decoded: Vec<serde_json::Value> = std::str::from_utf8(&raw)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(decoded, items);
            let repeated = write_foreach_snapshot(&root, "items", &items).unwrap();
            assert_eq!(repeated.path, written.path);
            let changed =
                write_foreach_snapshot(&root, "items", &[json!({"id":"different"})]).unwrap();
            assert_ne!(changed.path, written.path);
            assert_eq!(std::fs::read(&written.path).unwrap(), raw);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
