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
