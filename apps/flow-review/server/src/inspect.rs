use crate::{
    import::{self, SourceRequest},
    presentation::{self, NodeRef},
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};
use trollm_core::{plan::hex_sha256, ExecutionPlan, TaskRegistry};
use trollm_uniffi::bundle::{BundleManifest, ManifestFile};

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub level: &'static str,
    pub code: String,
    pub message: String,
    pub path: String,
}
impl Diagnostic {
    pub fn warning(code: &str, message: &str, path: &str) -> Self {
        Self {
            level: "warning",
            code: code.into(),
            message: message.chars().take(4096).collect(),
            path: path.into(),
        }
    }
    fn error(code: &str, message: &str, path: &str) -> Self {
        Self {
            level: "error",
            ..Self::warning(code, message, path)
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub format: &'static str,
    pub package_id: String,
    pub name: String,
    pub entry_flow: Option<String>,
    pub task_registry: Option<String>,
    pub candidates: Candidates,
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub files: Vec<Value>,
    pub plans: Vec<Value>,
    pub registry: Option<Value>,
    pub setup: Option<Value>,
    pub presentation: Option<Value>,
    pub unassigned_nodes: Vec<NodeRef>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidates {
    pub entry_flows: Vec<String>,
    pub task_registries: Vec<String>,
}

/// Resolve authored references lexically against package files before any core
/// loader can touch the filesystem. Parent traversal is allowed only within root.
fn reference(base_file: &str, relative: &str, files: &BTreeMap<String, String>) -> Result<String> {
    if relative.contains(['\\', '\0', ':']) || Path::new(relative).is_absolute() {
        bail!("unsafe source reference");
    }
    let mut parts: Vec<String> = Path::new(base_file)
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect();
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(p) => parts.push(p.to_str().context("non-UTF8 reference")?.into()),
            Component::CurDir => (),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    bail!("source reference escapes package");
                }
            }
            _ => bail!("unsafe source reference"),
        }
    }
    let path = parts.join("/");
    if !files.contains_key(&path) {
        bail!("source reference is missing: {path}");
    }
    Ok(path)
}

fn preflight(
    files: &BTreeMap<String, String>,
    parsed: &BTreeMap<String, ExecutionPlan>,
    registry_path: Option<&str>,
) -> Result<()> {
    let mut graph = BTreeMap::<String, Vec<String>>::new();
    for (path, plan) in parsed {
        let mut refs = Vec::new();
        for node in plan.nodes.values() {
            for r in [&node.body, &node.between].into_iter().flatten() {
                // flowRef is an ID resolved by the existing loader, not a path.
                if r.flow_ref.contains(['/', '\\']) || r.flow_ref == "." || r.flow_ref == ".." {
                    bail!("unsafe flow reference");
                }
                refs.push(reference(
                    path,
                    &format!("{}.flow.yaml", r.flow_ref),
                    files,
                )?);
            }
        }
        if let Some(setup) = &plan.setup {
            reference("root.flow.yaml", &setup.path, files)?;
        }
        graph.insert(path.clone(), refs);
    }
    fn depth(
        path: &str,
        graph: &BTreeMap<String, Vec<String>>,
        active: &mut BTreeSet<String>,
        count: usize,
        remaining: &mut usize,
    ) -> Result<()> {
        if count > 32 {
            bail!("subflow nesting exceeds 32");
        }
        if *remaining == 0 {
            bail!("subflow graph expansion exceeds limit");
        }
        *remaining -= 1;
        if !active.insert(path.into()) {
            return Ok(());
        }
        if let Some(children) = graph.get(path) {
            for child in children {
                depth(child, graph, active, count + 1, remaining)?;
            }
        }
        active.remove(path);
        Ok(())
    }
    for children in graph.values_mut() {
        children.sort();
        children.dedup();
    }
    let mut remaining = 50000;
    for path in graph.keys() {
        depth(path, &graph, &mut BTreeSet::new(), 0, &mut remaining)?;
    }
    if let Some(path) = registry_path {
        let registry =
            TaskRegistry::from_yaml_str(files.get(path).context("missing registry")?, ".")?;
        for task in registry.tasks.values() {
            reference(path, &task.prompt, files)?;
        }
    }
    Ok(())
}

pub fn inspect(request: SourceRequest, temp_root: &Path, display_name: &str) -> Result<Inspection> {
    let files = import::normalize(request.files)?;
    for selection in [&request.entry_flow, &request.task_registry]
        .into_iter()
        .flatten()
    {
        import::safe_path(selection)?;
    }
    let mut result = Inspection {
        format: "trollm.flow-review.v1",
        package_id: import::identity(&files),
        name: display_name.into(),
        entry_flow: None,
        task_registry: None,
        candidates: Candidates {
            entry_flows: Vec::new(),
            task_registries: Vec::new(),
        },
        valid: false,
        diagnostics: Vec::new(),
        files: Vec::new(),
        plans: Vec::new(),
        registry: None,
        setup: None,
        presentation: None,
        unassigned_nodes: Vec::new(),
    };
    let mut loaded_paths = BTreeSet::new();
    let mut parsed = BTreeMap::new();
    let mut node_count = 0;
    for (path, content) in &files {
        result.files.push(json!({ "path": path, "content": content, "sha256": hex_sha256(content.as_bytes()), "sizeBytes": content.len() }));
        if path.ends_with(".flow.yaml") {
            result.candidates.entry_flows.push(path.clone());
            match ExecutionPlan::from_yaml_str(content) {
                Ok(plan) => {
                    if parsed
                        .values()
                        .any(|prior: &ExecutionPlan| prior.id == plan.id)
                    {
                        result.diagnostics.push(Diagnostic::error(
                            "plan-duplicate-id",
                            "Flow IDs must identify one source plan in a package",
                            path,
                        ));
                    }
                    node_count += plan.nodes.len();
                    if parsed.len() >= 256 || node_count > 10000 {
                        bail!("package exceeds plan/node limits");
                    }
                    let mut value = serde_json::to_value(&plan)?;
                    value["path"] = json!(path);
                    result.plans.push(value);
                    parsed.insert(path.clone(), plan);
                }
                Err(error) => result.diagnostics.push(Diagnostic::error(
                    "plan-parse",
                    &format!("Flow parse: {error}"),
                    path,
                )),
            }
        } else if path.ends_with(".tasks.yaml") {
            result.candidates.task_registries.push(path.clone());
        }
    }
    let manifest: Option<BundleManifest> = if let Some(text) = files.get("manifest.json") {
        match serde_json::from_str(text) {
            Ok(m) => Some(m),
            Err(_) => {
                result.diagnostics.push(Diagnostic::error(
                    "manifest-invalid",
                    "Manifest does not match the bundle format",
                    "manifest.json",
                ));
                None
            }
        }
    } else {
        None
    };
    if let Some(manifest) = &manifest {
        // Original manifest bytes/hashes are authoritative; never repair pins.
        import::safe_path(&manifest.entry_flow)?;
        import::safe_path(&manifest.task_registry)?;
        let mut declared = BTreeSet::new();
        let mut valid = manifest.format == trollm_uniffi::bundle::BUNDLE_FORMAT;
        for f in &manifest.files {
            import::safe_path(&f.path)?;
            valid &= declared.insert(f.path.clone());
            valid &= files
                .get(&f.path)
                .is_some_and(|s| hex_sha256(s.as_bytes()).eq_ignore_ascii_case(&f.sha256));
        }
        valid &= files
            .keys()
            .filter(|p| p.as_str() != "manifest.json")
            .all(|p| declared.contains(p));
        for (path, plan) in &parsed {
            let name = Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if name != format!("{}.flow.yaml", plan.id) {
                valid = false;
            }
        }
        if !valid {
            result.diagnostics.push(Diagnostic::error(
                "manifest-integrity",
                "Manifest membership, format, flow filenames or SHA-256 verification failed",
                "manifest.json",
            ));
        }
        if request
            .entry_flow
            .as_ref()
            .is_some_and(|s| s != &manifest.entry_flow)
            || request
                .task_registry
                .as_ref()
                .is_some_and(|s| s != &manifest.task_registry)
        {
            result.diagnostics.push(Diagnostic::error(
                "manifest-selection",
                "Selection conflicts with the package manifest",
                "manifest.json",
            ));
        }
    }
    let choose = |explicit: Option<String>,
                  manifest: Option<String>,
                  candidates: &[String]|
     -> Option<String> {
        explicit.or(manifest).or_else(|| {
            if candidates.len() == 1 {
                Some(candidates[0].clone())
            } else {
                None
            }
        })
    };
    let mut roots = result.candidates.entry_flows.clone();
    let referenced: BTreeSet<_> = parsed
        .iter()
        .flat_map(|(path, p)| {
            p.nodes.values().flat_map(move |n| {
                [&n.body, &n.between].into_iter().flatten().map(move |r| {
                    Path::new(path)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .join(format!("{}.flow.yaml", r.flow_ref))
                        .to_string_lossy()
                        .into_owned()
                })
            })
        })
        .collect();
    roots.retain(|path| !referenced.contains(path));
    result.entry_flow = choose(
        request.entry_flow,
        manifest.as_ref().map(|m| m.entry_flow.clone()),
        &roots,
    );
    result.task_registry = choose(
        request.task_registry,
        manifest.as_ref().map(|m| m.task_registry.clone()),
        &result.candidates.task_registries,
    );
    if let Some(path) = &result.entry_flow {
        if !parsed.contains_key(path) {
            result.diagnostics.push(Diagnostic::error(
                "entry-selection",
                "Selected entry must be an included, parseable .flow.yaml source candidate",
                path,
            ));
        }
    }
    if let Some(path) = &result.task_registry {
        match files
            .get(path)
            .and_then(|s| TaskRegistry::from_yaml_str(s, ".").ok())
        {
            Some(registry) => {
                let mut value = serde_json::to_value(registry)?;
                value["path"] = json!(path);
                result.registry = Some(value);
            }
            None => {
                let message = match files.get(path) {
                    Some(text) => TaskRegistry::from_yaml_str(text, ".")
                        .err()
                        .map(|e| format!("Registry parse: {e}"))
                        .unwrap_or_default(),
                    None => "Selected registry file is missing".into(),
                };
                result
                    .diagnostics
                    .push(Diagnostic::error("registry-parse", &message, path));
            }
        }
    }
    if result.entry_flow.is_none() || result.task_registry.is_none() {
        result.diagnostics.push(Diagnostic::error(
            "selection-required",
            "Select an entry flow and task registry from the candidates",
            "",
        ));
    }
    // A private directory is owned by this call; Drop cleans every success/error.
    fs::create_dir_all(temp_root)?;
    let stage = tempfile::Builder::new()
        .prefix("inspection-")
        .tempdir_in(temp_root)?;
    let safe = preflight(&files, &parsed, result.task_registry.as_deref());
    if let Err(error) = &safe {
        result.diagnostics.push(Diagnostic::error(
            "source-reference",
            &format!("Source reference: {error}"),
            "",
        ));
    }
    if safe.is_ok() && !result.diagnostics.iter().any(|d| d.level == "error") {
        for (path, content) in &files {
            let target = stage.path().join(path);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(target, content)?;
        }
        if let (Some(entry), Some(registry)) = (&result.entry_flow, &result.task_registry) {
            let generated;
            let check_manifest = if let Some(m) = &manifest {
                m
            } else {
                generated = BundleManifest {
                    format: trollm_uniffi::bundle::BUNDLE_FORMAT.into(),
                    entry_flow: entry.clone(),
                    task_registry: registry.clone(),
                    files: files
                        .iter()
                        .map(|(path, text)| ManifestFile {
                            path: path.clone(),
                            sha256: hex_sha256(text.as_bytes()),
                        })
                        .collect(),
                };
                &generated
            };
            let validation = (|| -> Result<()> {
                let (plan, registry, setup) = trollm_uniffi::bundle::inspect_materialized_bundle(
                    stage.path(),
                    check_manifest,
                )?;
                fn collect_loaded(plan: &ExecutionPlan, base: &Path, paths: &mut BTreeSet<String>) {
                    if let Some(path) = plan
                        .source_path
                        .as_ref()
                        .and_then(|path| path.strip_prefix(base).ok())
                    {
                        paths.insert(path.to_string_lossy().into_owned());
                    }
                    for child in plan.subflows.values() {
                        collect_loaded(child, base, paths);
                    }
                }
                collect_loaded(&plan, &stage.path().canonicalize()?, &mut loaded_paths);
                trollm_core::validate::validate_authoring_provenance(&plan, &registry)?;
                trollm_core::validate::validate_plan(&plan)?;
                trollm_core::validate::validate_with_registry(&plan, &registry)?;
                result.setup = setup.map(serde_json::to_value).transpose()?;
                Ok(())
            })();
            match validation {
                Ok(()) => result.valid = true,
                Err(error) => {
                    let message = format!("{error:#}");
                    let message = if let Ok(canonical) = stage.path().canonicalize() {
                        message.replace(canonical.to_string_lossy().as_ref(), "<package>")
                    } else {
                        message
                    };
                    let message =
                        message.replace(stage.path().to_string_lossy().as_ref(), "<package>");
                    let message: String = message.chars().take(4096).collect();
                    result.diagnostics.push(Diagnostic::error(
                        "execution-validation",
                        &message,
                        entry,
                    ));
                }
            }
        }
    }
    if result.valid {
        for path in parsed.keys().filter(|path| !loaded_paths.contains(*path)) {
            result.diagnostics.push(Diagnostic::warning(
                "plan-unselected",
                "This supplied flow is outside the selected entry's subflow graph; its execution contract was not validated",
                path,
            ));
        }
    }
    let nodes: BTreeSet<NodeRef> = parsed
        .values()
        .flat_map(|p| {
            p.nodes.keys().map(move |n| NodeRef {
                flow: p.id.clone(),
                node: n.clone(),
            })
        })
        .collect();
    (result.presentation, result.unassigned_nodes) = presentation::check(
        files.get("flow-review.json"),
        &nodes,
        &mut result.diagnostics,
    );
    if let Some(title) = result
        .presentation
        .as_ref()
        .and_then(|p| p.get("title"))
        .and_then(Value::as_str)
    {
        if !title.trim().is_empty() {
            result.name = title.into();
        }
    }
    Ok(result)
}
