//! Flow bundle install (docs/ARCHITECTURE-ANDROID.md §8): a downloaded bundle
//! is an uncompressed ustar tar containing `manifest.json`, the flow(s), the
//! task registry and prompt files. Every file is sha256-verified against the
//! manifest (fail closed); the plan must pass core validation against the
//! bundled registry; content lands in the §8 layout.
//!
//! ```text
//! trollm/
//!   flows/<sha256>.flow.yaml  (+ .sig — tech debt #1)   canonical pins (§8)
//!   tasks/<sha256>.tasks.yaml (+ .sig)
//!   bundles/<rootFlowSha256>/   manifest.json, <id>.flow.yaml, prompts/, ...
//! ```
//!
//! The `bundles/<sha>/` materialized copy exists because core's plan loader
//! resolves `flowRef`s from sibling `<planId>.flow.yaml` files and the
//! registry resolves prompts relative to the registry file; the canonical
//! §8 `flows/`/`tasks/` files remain the content-addressed pins.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use trollm_core::plan::hex_sha256;
use trollm_core::{ExecutionPlan, TaskRegistry};

use crate::{tar, TrollmError};

pub const BUNDLE_FORMAT: &str = "trollm.flow-bundle.v1";
pub const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub format: String,
    /// Bundle-relative path of the entry (root) flow, `<id>.flow.yaml`.
    pub entry_flow: String,
    /// Bundle-relative path of the task registry, `<id>.tasks.yaml`.
    pub task_registry: String,
    /// Every file in the bundle (except the manifest itself) with its sha256.
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
}

pub struct InstallOutcome {
    pub installation_sha256: String,
    pub archive_sha256: String,
    pub flow_sha256: String,
    pub flow_id: String,
    pub flow_version: u32,
    pub files_verified: u32,
    pub bundle_dir: PathBuf,
    pub signature_status: String,
}

/// Verify and install a bundle under `base` (the `trollm/` dir, §8).
pub fn install_bundle(base: &Path, bytes: &[u8]) -> Result<InstallOutcome> {
    install_bundle_revision(base, bytes, false)
}

/// Owned-source imports retain a dependency-complete manifest identity. Legacy
/// manual installations remain addressed by their original entry-flow hash.
pub fn install_source_bundle(base: &Path, bytes: &[u8]) -> Result<InstallOutcome> {
    install_bundle_revision(base, bytes, true)
}

fn install_bundle_revision(base: &Path, bytes: &[u8], source: bool) -> Result<InstallOutcome> {
    let entries = tar::extract(bytes)?;
    let by_path: BTreeMap<String, &[u8]> = entries
        .iter()
        .map(|e| (e.path.clone(), e.data.as_slice()))
        .collect();

    let manifest_bytes = by_path
        .get(MANIFEST_NAME)
        .ok_or_else(|| anyhow!("bundle has no {MANIFEST_NAME}"))?;
    let manifest: BundleManifest =
        serde_json::from_slice(manifest_bytes).context("parsing bundle manifest")?;
    if manifest.format != BUNDLE_FORMAT {
        bail!(
            "bundle format must be '{BUNDLE_FORMAT}', got '{}'",
            manifest.format
        );
    }

    // --- Hash verification, fail closed in both directions. ---
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    for f in &manifest.files {
        if declared.insert(f.path.clone(), f.sha256.clone()).is_some() {
            bail!("manifest lists '{}' twice", f.path);
        }
    }
    for (path, expected) in &declared {
        let data = by_path
            .get(path)
            .ok_or_else(|| anyhow!("manifest file '{path}' missing from archive"))?;
        let actual = hex_sha256(data);
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("sha256 mismatch for '{path}': manifest {expected}, actual {actual}");
        }
    }
    for path in by_path.keys() {
        if path != MANIFEST_NAME && !declared.contains_key(path) {
            bail!("archive member '{path}' is not listed in the manifest");
        }
    }

    let entry_bytes = *by_path
        .get(&manifest.entry_flow)
        .ok_or_else(|| anyhow!("entryFlow '{}' not in bundle", manifest.entry_flow))?;
    let _registry_bytes = *by_path
        .get(&manifest.task_registry)
        .ok_or_else(|| anyhow!("taskRegistry '{}' not in bundle", manifest.task_registry))?;
    let root_sha = hex_sha256(entry_bytes);

    // Flow files must be named `<planId>.flow.yaml`: core's loader resolves
    // `flowRef` siblings by plan id.
    for (path, data) in &by_path {
        if path.ends_with(".flow.yaml") {
            let plan = ExecutionPlan::from_yaml_str(std::str::from_utf8(data)?)
                .with_context(|| format!("parsing flow '{path}'"))?;
            let stem = Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim_end_matches(".flow");
            if plan.id != stem {
                bail!("flow file '{path}' contains plan id '{}' — file must be named '<id>.flow.yaml'", plan.id);
            }
        }
    }

    // --- Stage, validate, publish. ---
    let installation_sha = hex_sha256(manifest_bytes);
    let archive_sha = hex_sha256(bytes);
    let bundles_dir = base.join(if source {
        "bundle-revisions"
    } else {
        "bundles"
    });
    let address = if source { &installation_sha } else { &root_sha };
    let final_dir = bundles_dir.join(address);
    if final_dir.exists() {
        // Content-addressed idempotency: an identical bundle reinstalls fine;
        // a different bundle for the same root flow sha is a conflict.
        let existing = fs::read(final_dir.join(MANIFEST_NAME)).unwrap_or_default();
        if hex_sha256(&existing) == hex_sha256(manifest_bytes) {
            validate_materialized_bundle(&final_dir, &manifest)?;
            // Publishing the directory and publishing discovery pins are
            // separate crash boundaries. Reinstall reconciles every pin.
            if !source {
                publish_pins(base, &by_path)?;
            }
            let mut out = outcome(
                base,
                &final_dir,
                &root_sha,
                manifest.files.len(),
                &installation_sha,
                &archive_sha,
            )?;
            out.signature_status = verify_bundle_signatures(&by_path)?;
            return Ok(out);
        }
        bail!("a different bundle is already installed for flow sha256 {root_sha}");
    }

    let staging = bundles_dir.join(format!(".staging-{address}"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for entry in &entries {
        let dest = staging.join(&entry.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, &entry.data)?;
    }

    // Fail closed on core validation: plan (incl. flowRef subflows, resolved
    // from the staged siblings) + registry (prompts resolve in the staged dir).
    let validation = (|| -> Result<(ExecutionPlan, TaskRegistry)> {
        let (plan, registry) = validate_materialized_bundle(&staging, &manifest)?;
        // New imports follow the authored provenance shape. Existing installed
        // bundles (including identical reinstalls above) keep the v1 read path.
        trollm_core::validate::validate_authoring_provenance(&plan, &registry)
            .map_err(|e| anyhow!(e.to_string()))?;
        trollm_core::validate::validate_plan(&plan).map_err(|e| anyhow!(e.to_string()))?;
        trollm_core::validate::validate_with_registry(&plan, &registry)
            .map_err(|e| anyhow!(e.to_string()))?;
        Ok((plan, registry))
    })();
    let (plan, _registry) = match validation {
        Ok(ok) => ok,
        Err(e) => {
            fs::remove_dir_all(&staging).ok();
            return Err(e.context("bundle plan failed core validation"));
        }
    };

    // Signature verification — SECURITY-ENTERPRISE.md tech-debt item #1.
    // HOOK ONLY, deliberately not implemented in v1: bundles are currently
    // authenticated solely by manifest sha256 checks above, so the transport
    // channel (enterprise MDM/USB side-load) is the trust root. To close the
    // debt: verify an ed25519/enterprise-PKI signature over the manifest here
    // and REJECT on failure; `.sig` files are already preserved below and in
    // the §8 layout so enabling this is not a format change.
    let signature_status = verify_bundle_signatures(&by_path)?;

    fs::create_dir_all(&bundles_dir)?;
    fs::rename(&staging, &final_dir)?;

    if !source {
        publish_pins(base, &by_path)?;
    }

    let mut out = outcome(
        base,
        &final_dir,
        &root_sha,
        manifest.files.len(),
        &installation_sha,
        &archive_sha,
    )?;
    out.flow_id = plan.id;
    out.flow_version = plan.version;
    out.signature_status = signature_status;
    Ok(out)
}

fn publish_pins(base: &Path, by_path: &BTreeMap<String, &[u8]>) -> Result<()> {
    // Canonical §8 pins: flows/<sha256>.flow.yaml, tasks/<sha256>.tasks.yaml,
    // with `.sig` sidecars when the bundle carries them.
    let flows_dir = base.join("flows");
    let tasks_dir = base.join("tasks");
    fs::create_dir_all(&flows_dir)?;
    fs::create_dir_all(&tasks_dir)?;
    for (path, data) in by_path {
        let sha = hex_sha256(data);
        if path.ends_with(".flow.yaml") {
            write_pin(
                &flows_dir,
                &sha,
                ".flow.yaml",
                data,
                by_path.get(&format!("{path}.sig")).copied(),
            )?;
        } else if path.ends_with(".tasks.yaml") {
            write_pin(
                &tasks_dir,
                &sha,
                ".tasks.yaml",
                data,
                by_path.get(&format!("{path}.sig")).copied(),
            )?;
        }
    }

    Ok(())
}

fn write_pin(dir: &Path, sha: &str, ext: &str, data: &[u8], sig: Option<&[u8]>) -> Result<()> {
    trollm_core::artifacts::write_atomic(dir, &format!("{sha}{ext}"), data)?;
    if let Some(sig) = sig {
        trollm_core::artifacts::write_atomic(dir, &format!("{sha}{ext}.sig"), sig)?;
    }
    Ok(())
}

/// Check the same manifest boundary both before publication and at startup.
/// Prompts may be nested or use `..` within the bundle, but their resolved
/// file must be one of the verified members and cannot escape via a symlink.
pub(crate) fn validate_materialized_bundle(
    dir: &Path,
    manifest: &BundleManifest,
) -> Result<(ExecutionPlan, TaskRegistry)> {
    let (plan, registry, _) = inspect_materialized_bundle(dir, manifest)?;
    Ok((plan, registry))
}

/// Inspect a staged package using the same integrity and reference boundary as
/// installation. This does not install, publish pins, load models or run tools.
/// Callers stage bounded regular files in a private directory and apply core
/// execution/provenance validation to the returned plan and registry.
pub fn inspect_materialized_bundle(
    dir: &Path,
    manifest: &BundleManifest,
) -> Result<(ExecutionPlan, TaskRegistry, Option<crate::setup::FlowSetup>)> {
    if manifest.format != BUNDLE_FORMAT {
        bail!("unsupported installed bundle format");
    }
    let root = dir.canonicalize()?;
    let mut verified = BTreeMap::new();
    for file in &manifest.files {
        let relative = Path::new(&file.path);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("manifest path '{}' must be bundle-relative", file.path);
        }
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(&root) {
            bail!("manifest path '{}' escapes the bundle", file.path);
        }
        if verified.insert(path, file.sha256.clone()).is_some() {
            bail!("duplicate resolved manifest path '{}'", file.path);
        }
        if !hex_sha256(&fs::read(root.join(relative))?).eq_ignore_ascii_case(&file.sha256) {
            bail!("installed bundle sha256 mismatch for '{}'", file.path);
        }
    }
    let verified_path = |relative: &str, base: &Path| -> Result<PathBuf> {
        if Path::new(relative).is_absolute() {
            bail!("bundle reference '{relative}' must be relative");
        }
        let path = base
            .join(relative)
            .canonicalize()
            .with_context(|| format!("resolving bundle reference '{relative}'"))?;
        if !path.starts_with(&root) || !verified.contains_key(&path) {
            bail!("bundle reference '{relative}' is outside the verified manifest");
        }
        Ok(path)
    };
    let flow_path = verified_path(&manifest.entry_flow, &root)?;
    let registry_path = verified_path(&manifest.task_registry, &root)?;
    let registry = TaskRegistry::from_yaml_file(&registry_path)?;
    for task in registry.tasks.values() {
        if let Some(prompt) = task.prompt_path() {
            verified_path(prompt, registry_path.parent().unwrap())?;
        }
    }
    let plan = ExecutionPlan::from_yaml_file(&flow_path)?;
    fn verify_flow_tree(
        plan: &ExecutionPlan,
        root: &Path,
        verified: &BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        let path = plan
            .source_path
            .as_ref()
            .ok_or_else(|| anyhow!("bundle flow has no source path"))?
            .canonicalize()?;
        if !path.starts_with(root) || !verified.contains_key(&path) {
            bail!("flow '{}' is outside the verified manifest", plan.id);
        }
        for child in plan.subflows.values() {
            verify_flow_tree(child, root, verified)?;
        }
        Ok(())
    }
    verify_flow_tree(&plan, &root, &verified)?;
    let setup = crate::setup::load_flow_setup(&plan, &root, &verified)?;
    Ok((plan, registry, setup))
}

/// Resolve an explicit full installation identity without changing legacy root pins.
pub(crate) fn revision_dir(base: &Path, installation: &str) -> Result<PathBuf> {
    if installation.len() != 64
        || !installation
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("installation sha256 must be 64 lowercase hexadecimal characters");
    }
    let dir = base.join("bundle-revisions").join(installation);
    if hex_sha256(&fs::read(dir.join(MANIFEST_NAME))?) != installation {
        bail!("installed manifest does not match installation identity");
    }
    Ok(dir)
}

pub(crate) fn revision_setup(
    base: &Path,
    installation: &str,
) -> Result<Option<crate::setup::FlowSetup>> {
    let dir = revision_dir(base, installation)?;
    let manifest: BundleManifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST_NAME))?)?;
    let (_, _, setup) = inspect_materialized_bundle(&dir, &manifest)?;
    Ok(setup)
}

/// Inspect setup using the same verified boundary as installation and startup.
pub(crate) fn installed_flow_setup(
    base: &Path,
    flow_sha256: &str,
) -> Result<Option<crate::setup::FlowSetup>> {
    if flow_sha256.len() != 64
        || !flow_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("flow sha256 must be 64 lowercase hexadecimal characters");
    }
    let dir = base.join("bundles").join(flow_sha256);
    let manifest_bytes = fs::read(dir.join(MANIFEST_NAME)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TrollmError::NotFound(format!("reading installed manifest: {error}"))
        } else {
            TrollmError::Io(format!("reading installed manifest: {error}"))
        }
    })?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)?;
    let (plan, _) = validate_materialized_bundle(&dir, &manifest)
        .map_err(|error| TrollmError::Bundle(format!("{error:#}")))?;
    if plan.sha256 != flow_sha256 {
        bail!("installed root flow does not match its content address");
    }
    // The first pass checked every manifest member; reconstruct the canonical
    // membership map for the shared descriptor loader, which rechecks its pin.
    let verified = manifest
        .files
        .iter()
        .map(|file| Ok((dir.join(&file.path).canonicalize()?, file.sha256.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    crate::setup::load_flow_setup(&plan, &dir, &verified)
}

fn outcome(
    base: &Path,
    bundle_dir: &Path,
    root_sha: &str,
    files: usize,
    installation_sha: &str,
    archive_sha: &str,
) -> Result<InstallOutcome> {
    // Re-read the installed entry flow for id/version (cheap; YAML is small).
    let manifest: BundleManifest = serde_json::from_slice(
        &fs::read(bundle_dir.join(MANIFEST_NAME)).context("reading installed manifest")?,
    )?;
    let plan = ExecutionPlan::from_yaml_file(&bundle_dir.join(&manifest.entry_flow))?;
    let _ = base;
    Ok(InstallOutcome {
        installation_sha256: installation_sha.to_string(),
        archive_sha256: archive_sha.to_string(),
        flow_sha256: root_sha.to_string(),
        flow_id: plan.id,
        flow_version: plan.version,
        files_verified: files as u32,
        bundle_dir: bundle_dir.to_path_buf(),
        signature_status: String::new(),
    })
}

/// TECH-DEBT #1 (docs/SECURITY-ENTERPRISE.md): signed content bundles.
/// **Not implemented.** Returns a status string instead of verifying so the
/// install path visibly reports that no signature check happened. When this
/// lands it must fail closed: unknown/absent signature → install rejected.
fn verify_bundle_signatures(_files: &BTreeMap<String, &[u8]>) -> Result<String> {
    Ok(
        "not-verified (signature verification deferred: SECURITY-ENTERPRISE.md tech-debt #1)"
            .to_string(),
    )
}
