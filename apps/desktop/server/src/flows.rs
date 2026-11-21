use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context};
use serde::Serialize;
use serde_json::{json, Value};
use trollm_core::{ExecutionPlan, TaskRegistry};
use trollm_uniffi::bundle::{self, BundleManifest, MANIFEST_NAME};
use trollm_uniffi::setup::FlowSetup;
use trollm_uniffi::tar::{self, MAX_BUNDLE_BYTES, MAX_BUNDLE_ENTRIES, MAX_MEMBER_BYTES};

const SOURCE_BYTES: usize = 512 * 1024;
const STRUCTURE_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlowDto {
    pub(crate) sha256: String,
    id: String,
    title: String,
    description: String,
    version: u32,
    pub(crate) setup: Option<FlowSetup>,
}

pub(crate) struct InstalledFlow {
    pub(crate) info: FlowDto,
    pub(crate) plan: ExecutionPlan,
    dir: PathBuf,
    manifest: BundleManifest,
    registry: TaskRegistry,
}

pub(crate) fn is_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn read_bounded(path: &Path, maximum: u64) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= maximum, "file exceeds its size limit");
    Ok(bytes)
}

// Owned-source envelopes require explicit source admission. The selected
// collection profile also uses an ordinary bundle manifest, so distinguish it
// by its generated profile member before passing bytes to the manual installer.
fn selected_member(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name == "collection-profile.json")
}

pub(crate) fn supports_manual_archive(bytes: &[u8]) -> anyhow::Result<bool> {
    let entries = tar::extract(bytes)?;
    Ok(!entries.iter().any(|entry| selected_member(&entry.path)))
}

impl InstalledFlow {
    pub(crate) fn load(base: &Path, sha: &str) -> anyhow::Result<Self> {
        ensure!(is_sha(sha), "invalid flow content address");
        let dir = base.join("bundles").join(sha);
        let manifest: BundleManifest =
            serde_json::from_slice(&read_bounded(&dir.join(MANIFEST_NAME), MAX_MEMBER_BYTES)?)?;
        ensure!(
            manifest.files.len() <= MAX_BUNDLE_ENTRIES,
            "too many bundle members"
        );
        let mut total = 0;
        for file in &manifest.files {
            let relative = Path::new(&file.path);
            ensure!(
                !relative.as_os_str().is_empty()
                    && relative
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_))),
                "invalid bundle member path"
            );
            let metadata = fs::symlink_metadata(dir.join(relative))?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_MEMBER_BYTES,
                "invalid bundle member"
            );
            total += metadata.len();
            ensure!(total <= MAX_BUNDLE_BYTES, "bundle exceeds its size limit");
        }
        let (plan, registry, setup) = bundle::inspect_materialized_bundle(&dir, &manifest)?;
        ensure!(
            plan.sha256 == sha,
            "installed flow content address mismatch"
        );
        Ok(Self {
            info: FlowDto {
                sha256: sha.to_owned(),
                id: plan.id.clone(),
                title: setup
                    .as_ref()
                    .map_or_else(|| plan.id.clone(), |setup| setup.title.clone()),
                description: setup
                    .as_ref()
                    .and_then(|setup| setup.description.clone())
                    .unwrap_or_default(),
                version: plan.version,
                setup,
            },
            plan,
            dir,
            manifest,
            registry,
        })
    }

    pub(crate) fn supported(&self) -> bool {
        !self
            .manifest
            .files
            .iter()
            .any(|file| selected_member(&file.path))
    }

    pub(crate) fn declarations(&self) -> anyhow::Result<(bool, BTreeMap<&str, &str>)> {
        fn collect<'a>(
            plan: &'a ExecutionPlan,
            servers: &mut BTreeMap<&'a str, &'a str>,
        ) -> anyhow::Result<bool> {
            let mut uses_models = !plan.models.is_empty();
            for server in &plan.mcp.servers {
                if servers
                    .insert(&server.name, &server.url)
                    .is_some_and(|previous| previous != server.url)
                {
                    bail!("conflicting MCP server URLs");
                }
            }
            for child in plan.subflows.values() {
                uses_models |= collect(child, servers)?;
            }
            Ok(uses_models)
        }

        let mut servers = BTreeMap::new();
        let uses_models = collect(&self.plan, &mut servers)?;
        Ok((uses_models, servers))
    }

    pub(crate) fn inspect(&self) -> anyhow::Result<Value> {
        let mut remaining_sources = SOURCE_BYTES;
        let mut omitted = false;
        let mut entry_flows = Vec::new();
        let mut task_registries = Vec::new();
        let mut files = Vec::new();
        let flow_paths = self
            .manifest
            .files
            .iter()
            .filter(|file| file.path.ends_with(".flow.yaml"))
            .map(|file| (file.sha256.to_ascii_lowercase(), file.path.as_str()))
            .collect::<BTreeMap<_, _>>();

        for file in &self.manifest.files {
            if file.path.ends_with(".flow.yaml") {
                entry_flows.push(&file.path);
            }
            if file.path.ends_with(".tasks.yaml") {
                task_registries.push(&file.path);
            }
            let path = self.dir.join(&file.path);
            let size = fs::metadata(&path)?.len();
            let content = if size <= remaining_sources as u64 {
                let bytes = read_bounded(&path, remaining_sources as u64)?;
                match String::from_utf8(bytes) {
                    Ok(text) => {
                        remaining_sources -= text.len();
                        Some(text)
                    }
                    Err(_) => None,
                }
            } else {
                omitted = true;
                None
            };
            files.push(json!({ "path": file.path, "sha256": file.sha256, "sizeBytes": size, "content": content }));
        }

        fn plans(
            plan: &ExecutionPlan,
            path: &str,
            paths: &BTreeMap<String, &str>,
            out: &mut Vec<Value>,
            remaining: &mut usize,
            omitted: &mut bool,
        ) -> anyhow::Result<()> {
            let mut value = serde_json::to_value(plan)?;
            value["path"] = json!(path);
            let size = serde_json::to_vec(&value)?.len();
            if size <= *remaining {
                out.push(value);
                *remaining -= size;
            } else {
                *omitted = true;
            }
            for child in plan.subflows.values() {
                plans(
                    child,
                    paths
                        .get(&child.sha256)
                        .context("missing verified subflow")?,
                    paths,
                    out,
                    remaining,
                    omitted,
                )?;
            }
            Ok(())
        }

        let mut remaining_structure = STRUCTURE_BYTES;
        let mut projected_plans = Vec::new();
        plans(
            &self.plan,
            &self.manifest.entry_flow,
            &flow_paths,
            &mut projected_plans,
            &mut remaining_structure,
            &mut omitted,
        )?;
        let mut registry = serde_json::to_value(&self.registry)?;
        registry["path"] = json!(self.manifest.task_registry);
        if serde_json::to_vec(&registry)?.len() > remaining_structure {
            registry = Value::Null;
            omitted = true;
        }
        let diagnostics = if omitted {
            vec![
                json!({"level":"warning", "code":"inspection-bounded", "message":"Some source details exceed the inspection limit.", "path":""}),
            ]
        } else {
            Vec::new()
        };

        Ok(json!({
            "format": "trollm.flow-review.v1",
            "packageId": self.info.sha256,
            "name": self.info.title,
            "entryFlow": self.manifest.entry_flow,
            "taskRegistry": self.manifest.task_registry,
            "candidates": { "entryFlows": entry_flows, "taskRegistries": task_registries },
            "valid": true,
            "diagnostics": diagnostics,
            "files": files,
            "plans": projected_plans,
            "registry": registry,
            "setup": self.info.setup,
            "presentation": null,
            "unassignedNodes": [],
        }))
    }
}
