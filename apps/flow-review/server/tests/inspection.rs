use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use trollm_flow_review::{
    import::{self, SourceFile, SourceRequest},
    inspect::inspect,
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}
fn sources(paths: &[&str]) -> Vec<SourceFile> {
    paths
        .iter()
        .map(|p| SourceFile {
            path: p.to_string(),
            content: fs::read_to_string(root().join(p)).unwrap(),
        })
        .collect()
}
fn minimal() -> Vec<SourceFile> {
    sources(&[
        "flows/examples/minimal-summary.flow.yaml",
        "flows/tasks/minimal-summary.tasks.yaml",
        "flows/tasks/prompts/minimal-summary.md",
    ])
}
fn review(files: Vec<SourceFile>) -> trollm_flow_review::inspect::Inspection {
    let temp = tempfile::tempdir().unwrap();
    let result = inspect(
        SourceRequest {
            files,
            ..Default::default()
        },
        temp.path(),
        "Test",
    )
    .unwrap();
    assert_eq!(
        fs::read_dir(temp.path()).unwrap().count(),
        0,
        "staged files cleaned"
    );
    result
}
#[test]
fn real_minimal_summary_and_order_independent_identity() {
    let mut files = minimal();
    let first = review(files.clone());
    assert!(first.valid, "{:?}", first.diagnostics);
    assert_eq!(first.plans.len(), 1);
    assert_eq!(
        first.registry.as_ref().unwrap()["tasks"]["summarize-text"]["model"],
        "coder"
    );
    assert!(first.plans[0]["input"]["promptTemplate"]
        .as_str()
        .unwrap()
        .contains("Summarize"));
    files.reverse();
    assert_eq!(first.package_id, review(files).package_id);
    let serialized = serde_json::to_string(&first).unwrap();
    assert!(!serialized.contains("sourcePath"));
    assert!(!serialized.contains("baseDir"));
    assert!(!serialized.contains("inspection-"));
}
#[test]
fn real_adaptive_routes() {
    let result = review(sources(&[
        "flows/examples/adaptive-routing.flow.yaml",
        "flows/tasks/adaptive-routing.tasks.yaml",
        "flows/tasks/prompts/adaptive-assess.md",
        "flows/tasks/prompts/adaptive-choose.md",
        "flows/tasks/prompts/adaptive-report.md",
    ]));
    assert!(result.valid, "{:?}", result.diagnostics);
    assert!(result.plans[0]["nodes"].as_object().unwrap().len() > 3);
}

#[test]
fn direct_only_source_needs_no_prompt_or_model_and_mixed_source_keeps_prompt_checks() {
    let flow = r#"
schemaVersion: ledger.execution-plan.v1
id: direct
version: 1
limits: {maxSteps: 1, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 30, artifactMaxMb: 1}
onLimit: abort
models: {}
entryStep: calculate
nodes:
  calculate: {kind: task, taskRef: add, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#;
    let registry = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  add:
    execution: tool
    completion: tool-result
    tools: [local.calculate]
    toolArguments:
      local.calculate:
        modelFields: []
        fixed: {operation: add, left: '1', right: '2'}
"#;
    let mut files = vec![
        SourceFile {
            path: "direct.flow.yaml".into(),
            content: flow.into(),
        },
        SourceFile {
            path: "direct.tasks.yaml".into(),
            content: registry.into(),
        },
    ];
    let direct = review(files.clone());
    assert!(direct.valid, "{:?}", direct.diagnostics);
    let task = &direct.registry.as_ref().unwrap()["tasks"]["add"];
    assert_eq!(task["execution"], "tool");
    assert!(task.get("model").is_none());
    assert!(task.get("prompt").is_none());

    files[0].content = flow
        .replace("models: {}", "models: {coder: {ctx: 2048}}")
        .replace(
            "taskRef: add, onVerified: done",
            "taskRef: add, onVerified: report",
        )
        .replace(
            "  done:",
            "  report: {kind: task, taskRef: report, onVerified: done}\n  done:",
        );
    files[1].content = format!("{registry}\n  report:\n    model: coder\n    prompt: report.md\n");
    assert!(
        !review(files.clone()).valid,
        "mixed model task still needs its prompt"
    );
    files.push(SourceFile {
        path: "report.md".into(),
        content: "Report the result.".into(),
    });
    let mixed = review(files);
    assert!(mixed.valid, "{:?}", mixed.diagnostics);
    assert_eq!(
        mixed.registry.as_ref().unwrap()["tasks"]["report"]["model"],
        "coder"
    );
}
#[test]
fn current_argument_contracts_and_artifact_views_survive_inspection() {
    let mut files = sources(&[
        "flows/starters/check-total/check-total.flow.yaml",
        "flows/starters/check-total/check-total.tasks.yaml",
        "flows/starters/check-total/setup.json",
        "flows/starters/check-total/prompts/sum.md",
        "flows/starters/check-total/prompts/compare.md",
        "flows/starters/check-total/prompts/advise.md",
        "flows/starters/check-total/prompts/report.md",
    ]);
    // Import the starter as its own package, whose setup path is rooted at the
    // package directory rather than the containing source checkout.
    for file in &mut files {
        file.path = file
            .path
            .strip_prefix("flows/starters/check-total/")
            .unwrap()
            .to_string();
    }
    let mut registry = serde_json::to_value(
        trollm_core::TaskRegistry::from_yaml_str(&files[1].content, ".").unwrap(),
    )
    .unwrap();
    registry["tasks"]["sum-cart"]["toolArguments"]["local.calculate"]["modelFields"] =
        serde_json::json!(["left", "right"]);
    registry["tasks"]["compare-budget"]["toolArguments"]["local.calculate"]["modelFields"] =
        serde_json::json!([]);
    registry["tasks"]["sum-cart"]["outputContract"]["outputs"]["compact"] = serde_json::json!({"fromArtifact":"calculation", "format":"json", "type":"object",
            "view":{"fields":{"operation":true,"result":true},"maxBytes":256}});
    files[1].content = registry.to_string();
    let result = review(files.clone());
    assert!(result.valid, "{:?}", result.diagnostics);
    let inspected = result.registry.as_ref().unwrap();
    assert_eq!(
        inspected["tasks"]["sum-cart"]["toolArguments"]["local.calculate"]["modelFields"],
        serde_json::json!(["left", "right"])
    );
    assert_eq!(
        inspected["tasks"]["sum-cart"]["outputContract"]["outputs"]["compact"]["view"],
        registry["tasks"]["sum-cart"]["outputContract"]["outputs"]["compact"]["view"]
    );
    // Invalid new contracts must remain visible as errors, not disappear during
    // the companion's parse/serialize/materialize validation path.
    registry["tasks"]["sum-cart"]["toolArguments"]["local.calculate"]["modelFields"] =
        serde_json::Value::Null;
    files[1].content = registry.to_string();
    assert!(!review(files).valid);
}
#[test]
fn unsafe_paths_duplicates_and_non_text_archives_rejected() {
    for path in [
        "../escape",
        "/absolute",
        "a/../b",
        "a//b",
        "a\\b",
        "C:bad",
        "a/./b",
        "",
    ] {
        assert!(
            import::normalize(vec![SourceFile {
                path: path.into(),
                content: "x".into()
            }])
            .is_err(),
            "{path}"
        );
    }
    assert!(import::normalize(vec![
        SourceFile {
            path: "a".into(),
            content: "1".into()
        },
        SourceFile {
            path: "a".into(),
            content: "2".into()
        }
    ])
    .is_err());
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::Char,
    ] {
        let mut builder = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_ustar();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_entry_type(kind);
        h.set_cksum();
        builder.append_data(&mut h, "bad", &b""[..]).unwrap();
        let bytes = builder.into_inner().unwrap();
        assert!(import::archive(&bytes).is_err());
    }
}
#[test]
fn manifests_and_setup_pins_fail_closed() {
    let mut files = minimal();
    let manifest = serde_json::json!({"format":"trollm.flow-bundle.v1","entryFlow":files[0].path,"taskRegistry":files[1].path,"files":files.iter().map(|f| serde_json::json!({"path":f.path,"sha256":trollm_core::plan::hex_sha256(f.content.as_bytes())})).collect::<Vec<_>>()});
    files.push(SourceFile {
        path: "manifest.json".into(),
        content: manifest.to_string(),
    });
    assert!(review(files.clone()).valid);
    files[2].content.push_str("changed");
    let invalid = review(files);
    assert!(!invalid.valid);
    assert!(invalid
        .diagnostics
        .iter()
        .any(|d| d.code == "manifest-integrity"));
    let mut files = minimal();
    files[0].content.push_str("\nsetup: {path: setup.json, sha256: '0000000000000000000000000000000000000000000000000000000000000000'}\n");
    files.push(SourceFile {
        path: "setup.json".into(),
        content: "{}".into(),
    });
    assert!(!review(files).valid);
}
#[test]
fn incomplete_ambiguous_and_escape_remain_inspectable() {
    let missing = review(vec![minimal()[0].clone()]);
    assert!(!missing.valid);
    assert_eq!(missing.files.len(), 1);
    let mut files = minimal();
    let mut extra = files[0].clone();
    extra.path = "other.flow.yaml".into();
    extra.content = extra.content.replace("id: minimal-summary", "id: other");
    files.push(extra);
    let r = review(files);
    assert!(!r.valid);
    assert!(r.entry_flow.is_none());
    assert_eq!(r.plans.len(), 2);
    let mut files = minimal();
    files[1].content = files[1]
        .content
        .replace("prompts/minimal-summary.md", "../../../../etc/passwd");
    let r = review(files);
    assert!(!r.valid);
    assert!(r.diagnostics.iter().any(|d| d.code == "source-reference"));
}
#[test]
fn presentation_errors_are_separate_and_unassigned_nodes_visible() {
    let mut files = minimal();
    files.push(SourceFile {path:"flow-review.json".into(),content:serde_json::json!({"format":"trollm.flow-presentation.v1","title":"Review","purpose":"Explain","phases":[{"id":"p","title":"P","description":"Description","nodes":[{"flow":"minimal-summary","node":"summarize"},{"flow":"minimal-summary","node":"missing"}]}]}).to_string()});
    let r = review(files);
    assert!(r.valid);
    assert!(r
        .diagnostics
        .iter()
        .any(|d| d.code == "presentation-unknown-node"));
    assert!(r.presentation.is_none());
    assert_eq!(r.unassigned_nodes.len(), 2);
}
#[test]
fn large_prompt_and_many_cyclic_nodes_are_bounded() {
    let mut files = minimal();
    files[2]
        .content
        .push_str(&"long source text\n".repeat(20000));
    let mut plan = trollm_core::ExecutionPlan::from_yaml_str(&files[0].content).unwrap();
    let node = plan.nodes["summarize"].clone();
    for n in 0..400 {
        let mut node = node.clone();
        node.on_verified = Some(trollm_core::plan::Transition::Node(format!(
            "task{}",
            (n + 1) % 400
        )));
        plan.nodes.insert(format!("task{n}"), node);
    }
    files[0].content = serde_json::to_string(&plan).unwrap();
    let r = review(files);
    assert!(r.valid, "{:?}", r.diagnostics);
    assert!(
        r.files
            .iter()
            .map(|f| f["sizeBytes"].as_u64().unwrap())
            .sum::<u64>()
            > 300000
    );
    assert_eq!(r.plans[0]["nodes"].as_object().unwrap().len(), 402);
}
#[test]
fn mapped_symlinks_are_rejected() {
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("secret")).unwrap();
        assert!(import::directory(dir.path()).is_err());
    }
}
#[test]
fn unused_files_change_identity() {
    let mut files = BTreeMap::new();
    files.insert("a".into(), "b".into());
    let first = import::identity(&files);
    files.insert("unused.md".into(), "extra".into());
    assert_ne!(first, import::identity(&files));
}
#[test]
fn real_foreach_subflows_pause_and_shared_prompts() {
    let r = review(sources(&[
        "flows/examples/ticket-triage.flow.yaml",
        "flows/examples/process-item.flow.yaml",
        "flows/examples/return-to-list.flow.yaml",
        "flows/tasks/tickets.tasks.yaml",
        "flows/tasks/prompts/prepare-worklist.md",
        "flows/tasks/prompts/enrich-ticket.md",
        "flows/tasks/prompts/consult-steering.md",
        "flows/tasks/prompts/execute-decision.md",
        "flows/tasks/prompts/progress-checkpoint.md",
        "flows/tasks/prompts/finalize-batch.md",
        "flows/tasks/prompts/finalize-empty-batch.md",
    ]));
    assert!(r.valid, "{:?}", r.diagnostics);
    assert_eq!(r.plans.len(), 3);
    assert_eq!(
        r.entry_flow.as_deref(),
        Some("flows/examples/ticket-triage.flow.yaml")
    );
    let root = r.plans.iter().find(|p| p["id"] == "ticket-triage").unwrap();
    assert_eq!(root["nodes"]["each"]["body"]["flowRef"], "process-item");
    assert_eq!(root["nodes"]["each"]["onLimit"]["status"], "paused");
    let mut files = minimal();
    let mut reg = trollm_core::TaskRegistry::from_yaml_str(&files[1].content, ".").unwrap();
    let task = reg.tasks["summarize-text"].clone();
    reg.tasks.insert("shared".into(), task);
    files[1].content = serde_json::to_string(&reg).unwrap();
    let r = review(files);
    assert!(r.valid);
    assert_eq!(
        r.registry.as_ref().unwrap()["tasks"]["shared"]["prompt"],
        "prompts/minimal-summary.md"
    );
}

#[test]
fn reject_shared_descendants_before_recursive_loading_but_allow_same_parent_reuse() {
    use serde_json::json;
    let each = |child: &str| {
        json!({
            "kind":"foreach", "items":{"from":"user_input"}, "itemKey":"id",
            "bindAs":"item", "enumeration":"snapshot", "ordering":"source",
            "body":{"flowRef":child},
            "outputs":{"results":{"collect":"body.outputs","indexBy":"itemKey"}},
            "onEmpty":"done", "onComplete":"done"
        })
    };
    let done = json!({"kind":"end","outcome":"succeeded"});
    let plan = |id: &str, nodes: serde_json::Value, entry: &str| {
        SourceFile {
        path: format!("{id}.flow.yaml"),
        content: json!({
            "schemaVersion":"ledger.execution-plan.v1", "id":id,"version":1,
            "limits":{"maxSteps":4,"maxReasonerCalls":0,"maxTokensTotal":4096,"timeoutS":60,"artifactMaxMb":1},
            "onLimit":"abort","models":{"coder":{"ctx":4096}},
            "entryStep":entry, "nodes":nodes
        }).to_string(),
    }
    };
    let registry = SourceFile {
        path: "r.tasks.yaml".into(),
        content: json!({"schemaVersion":"ledger.task-registry.v1","tasks":{}}).to_string(),
    };
    let leaf = plan("leaf", json!({"done":done}), "done");
    // Core deduplicates references within one parent. These remain valid.
    let same_parent = review(vec![
        plan(
            "root",
            json!({"first":each("leaf"),"second":each("leaf"),"done":done}),
            "first",
        ),
        leaf.clone(),
        registry.clone(),
    ]);
    assert!(same_parent.valid, "{:?}", same_parent.diagnostics);
    let shared = review(vec![
        plan(
            "root",
            json!({"left":each("left"),"right":each("right"),"done":done}),
            "left",
        ),
        plan("left", json!({"each":each("leaf"),"done":done}), "each"),
        plan("right", json!({"each":each("leaf"),"done":done}), "each"),
        leaf,
        registry.clone(),
    ]);
    assert!(!shared.valid);
    assert!(shared
        .diagnostics
        .iter()
        .any(|d| d.code == "source-reference" && d.message.contains("multiple parents")));
    assert!(
        !shared
            .diagnostics
            .iter()
            .any(|d| d.code == "execution-validation"),
        "must fail before recursive loader"
    );
    let cycle = review(vec![
        plan("root", json!({"each":each("child"),"done":done}), "each"),
        plan("child", json!({"each":each("root"),"done":done}), "each"),
        registry,
    ]);
    assert!(!cycle.valid);
    assert!(cycle
        .diagnostics
        .iter()
        .any(|d| d.code == "source-reference" && d.message.contains("cyclic")));
}
#[test]
fn real_starter_setup_and_tar_roundtrip() {
    let directory = root().join("flows/starters/check-total");
    let files = import::directory(&directory).unwrap();
    let r = review(
        files
            .iter()
            .map(|(path, content)| SourceFile {
                path: path.clone(),
                content: content.clone(),
            })
            .collect(),
    );
    assert!(r.valid, "{:?}", r.diagnostics);
    assert!(r.setup.is_some());
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in &files {
        let mut h = tar::Header::new_ustar();
        h.set_size(content.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder
            .append_data(&mut h, path, content.as_bytes())
            .unwrap();
    }
    let archive = builder.into_inner().unwrap();
    let decoded = import::archive(&archive).unwrap();
    assert_eq!(files, decoded);
}
#[test]
fn raw_archive_absolute_duplicate_and_parent_entries_rejected() {
    fn tar_file(path: &str) -> Vec<u8> {
        let mut h = tar::Header::new_ustar();
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        let mut bytes = h.as_bytes().to_vec();
        bytes.push(b'x');
        bytes.resize(1024, 0);
        bytes.resize(2048, 0);
        bytes
    }
    for path in ["/absolute", "../escape", "a//b", "a/./b"] {
        assert!(import::archive(&tar_file(path)).is_err(), "{path}");
    }
    let mut bytes = tar_file("duplicate");
    bytes.truncate(1024);
    bytes.extend(tar_file("duplicate"));
    assert!(import::archive(&bytes).is_err());
    let mut bytes = tar_file("file");
    bytes[0] = b'X';
    assert!(import::archive(&bytes).is_err(), "header checksum");
}
#[test]
fn presentation_unknown_fields_duplicate_membership_and_bounds_are_diagnostics() {
    for text in [
        serde_json::json!({"format":"trollm.flow-presentation.v1","title":"T","purpose":"P","phases":[],"executable":true}),
        serde_json::json!({"format":"trollm.flow-presentation.v1","title":"T","purpose":"P","phases":[{"id":"p","title":"P","description":"D","nodes":[{"flow":"minimal-summary","node":"done"},{"flow":"minimal-summary","node":"done"}]}]}),
    ] {
        let mut files = minimal();
        files.push(SourceFile {
            path: "flow-review.json".into(),
            content: text.to_string(),
        });
        let r = review(files);
        assert!(r.valid);
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.code == "presentation-invalid" || d.code == "presentation-duplicate-node"));
    }
    let mut files = minimal();
    let mut extra = files[0].clone();
    extra.path = "name-mismatch.flow.yaml".into();
    files[0] = extra;
    let manifest = serde_json::json!({"format":"trollm.flow-bundle.v1","entryFlow":files[0].path,"taskRegistry":files[1].path,"files":files.iter().map(|f| serde_json::json!({"path":f.path,"sha256":trollm_core::plan::hex_sha256(f.content.as_bytes())})).collect::<Vec<_>>()});
    files.push(SourceFile {
        path: "manifest.json".into(),
        content: manifest.to_string(),
    });
    assert!(!review(files).valid);
}
#[test]
fn excessive_depth_and_files_are_rejected_without_loading() {
    let too_many = (0..1025)
        .map(|n| SourceFile {
            path: format!("{n}.md"),
            content: String::new(),
        })
        .collect();
    assert!(import::normalize(too_many).is_err());
    let deep = format!("{}file.md", "x/".repeat(33));
    assert!(import::safe_path(&deep).is_err());
    let mut files = minimal();
    let template = files[0].content.clone();
    for n in 0..34 {
        let mut plan = trollm_core::ExecutionPlan::from_yaml_str(&template).unwrap();
        plan.id = format!("flow{n}");
        let mut node = plan.nodes["summarize"].clone();
        node.body = Some(trollm_core::plan::FlowRef {
            flow_ref: format!("flow{}", n + 1),
            inputs: Default::default(),
        });
        plan.nodes.insert("summarize".into(), node);
        if n == 33 {
            plan.nodes.get_mut("summarize").unwrap().body = None;
        }
        files.push(SourceFile {
            path: format!("flows/examples/flow{n}.flow.yaml"),
            content: serde_json::to_string(&plan).unwrap(),
        });
    }
    let temp = tempfile::tempdir().unwrap();
    let r = inspect(
        SourceRequest {
            files,
            entry_flow: Some("flows/examples/flow0.flow.yaml".into()),
            ..Default::default()
        },
        temp.path(),
        "Deep",
    )
    .unwrap();
    assert!(!r.valid);
    assert!(r.diagnostics.iter().any(|d| d.code == "source-reference"));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn optional_presentation_descriptions_accept_omission_but_reject_null() {
    for field in ["inputsDescription", "resultDescription"] {
        let mut value = serde_json::json!({"format":"trollm.flow-presentation.v1","title":"Review","purpose":"Explain","phases":[]});
        let mut files = minimal();
        files.push(SourceFile {
            path: "flow-review.json".into(),
            content: value.to_string(),
        });
        assert!(review(files).presentation.is_some());
        value[field] = serde_json::Value::Null;
        let mut files = minimal();
        files.push(SourceFile {
            path: "flow-review.json".into(),
            content: value.to_string(),
        });
        let result = review(files);
        assert!(result.valid);
        assert!(result.presentation.is_none());
        assert_eq!(result.unassigned_nodes.len(), 2);
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "presentation-invalid"));
    }
}

#[test]
fn unused_supplied_flow_stays_visible_with_explicit_validation_scope() {
    let mut files = minimal();
    let mut unused = files[0].clone();
    unused.path = "flows/examples/unused.flow.yaml".into();
    unused.content = unused
        .content
        .replace("id: minimal-summary", "id: unused")
        .replace("taskRef: summarize-text", "taskRef: missing-task");
    files.push(unused);
    let temp = tempfile::tempdir().unwrap();
    let result = inspect(
        SourceRequest {
            files,
            entry_flow: Some("flows/examples/minimal-summary.flow.yaml".into()),
            ..Default::default()
        },
        temp.path(),
        "Scope",
    )
    .unwrap();
    assert!(result.valid, "{:?}", result.diagnostics);
    assert_eq!(result.plans.len(), 2);
    assert!(result
        .unassigned_nodes
        .iter()
        .any(|node| node.flow == "unused"));
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.code == "plan-unselected")
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].path, "flows/examples/unused.flow.yaml");
}

#[test]
fn non_flow_entry_selection_cannot_bypass_reference_preflight() {
    let mut files = minimal();
    let mut disguised = trollm_core::ExecutionPlan::from_yaml_str(&files[0].content).unwrap();
    disguised.nodes.get_mut("summarize").unwrap().body = Some(trollm_core::plan::FlowRef {
        flow_ref: "/outside/package/private".into(),
        inputs: Default::default(),
    });
    files.push(SourceFile {
        path: "disguised.md".into(),
        content: serde_json::to_string(&disguised).unwrap(),
    });
    let temp = tempfile::tempdir().unwrap();
    let result = inspect(
        SourceRequest {
            files,
            entry_flow: Some("disguised.md".into()),
            ..Default::default()
        },
        temp.path(),
        "Disguised",
    )
    .unwrap();
    assert!(!result.valid);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == "entry-selection"));
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.code == "execution-validation"),
        "canonical filesystem loader was never entered"
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn preserves_explicit_child_exports_and_foreach_collection_views() {
    use serde_json::json;
    let mut files = minimal();
    let root = files
        .iter_mut()
        .find(|file| file.path.ends_with(".flow.yaml"))
        .unwrap();
    root.content
        .push_str("\nexports:\n  summary: {from: summarize.outputs.summary}\n");
    let result = review(files);
    assert!(result.valid, "{:?}", result.diagnostics);
    assert_eq!(
        result.plans[0]["exports"]["summary"]["from"],
        "summarize.outputs.summary"
    );
    // Foreach view parsing is the same typed core contract used by inspection.
    let collect: trollm_core::plan::ForeachCollect = serde_json::from_value(json!({
        "collect":"body.outputs", "indexBy":"itemKey",
        "view":{"fields":{"outputs":{"amount":true}},"maxBytes":4096}
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(collect).unwrap()["view"]["maxBytes"],
        4096
    );
}
