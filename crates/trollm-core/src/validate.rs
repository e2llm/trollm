//! Plan + registry validation (§4.2). Everything statically checkable is a
//! load-time error here; what remains (e.g. a root `end` outcome outside
//! succeeded/failed) fails at runtime with `unhandled-transition`.

use crate::error::ValidationError;
use crate::plan::*;
use crate::registry::TaskRegistry;

pub fn validate_plan(plan: &ExecutionPlan) -> Result<(), ValidationError> {
    let mut problems = Vec::new();
    fn unique_ids(
        plan: &ExecutionPlan,
        seen: &mut std::collections::BTreeSet<String>,
        problems: &mut Vec<String>,
    ) {
        if !seen.insert(plan.id.clone()) {
            problems.push(format!("ambiguous duplicate plan id '{}'", plan.id));
        }
        for sub in plan.subflows.values() {
            unique_ids(sub, seen, problems);
        }
    }
    unique_ids(plan, &mut std::collections::BTreeSet::new(), &mut problems);
    validate_one(plan, &mut problems);
    for sub in plan.subflows.values() {
        validate_plan_into(sub, &mut problems);
    }
    if !problems.is_empty() {
        return Err(ValidationError::new(
            format!("execution plan '{}'", plan.id),
            problems,
        ));
    }
    Ok(())
}

fn validate_plan_into(plan: &ExecutionPlan, problems: &mut Vec<String>) {
    if plan.setup.is_some() {
        problems.push("setup is supported only on the root flow, not on subflows".into());
    }
    validate_one(plan, problems);
    for sub in plan.subflows.values() {
        validate_plan_into(sub, problems);
    }
}

fn validate_one(plan: &ExecutionPlan, problems: &mut Vec<String>) {
    let at = |node: &str| format!("plan '{}' node '{node}'", plan.id);

    if let Some(exports) = &plan.exports {
        let identifier = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        };
        if exports.is_empty()
            || exports.len() > 32
            || exports.iter().any(|(name, binding)| {
                !identifier(name)
                    || binding.from.len() > 256
                    || !binding.from.split('.').all(identifier)
            })
        {
            problems.push(format!("plan '{}': exports requires 1 through 32 named static dotted bindings (names <=64, paths <=256 bytes)", plan.id));
        }
    }

    // Policies are opaque in the plan model, but their authored keys are closed
    // by the gateway's typed model, including when a custom gateway is supplied.
    let policies = serde_yaml::to_value(&plan.mcp.policies)
        .map_err(anyhow::Error::from)
        .and_then(|value| trollm_mcp::PolicySet::from_yaml_value(&value));
    if let Err(error) = policies {
        problems.push(format!("plan '{}': {error:#}", plan.id));
    }

    if plan.mcp.servers.iter().any(|s| s.name == "flow")
        || plan.mcp.policies.keys().any(|k| k.starts_with("flow."))
    {
        problems.push("flow is a reserved runtime namespace, not a remote server or policy".into());
    }
    if let Some(setup) = &plan.setup {
        if let Err(problem) = setup.validate() {
            problems.push(problem);
        }
    }

    if plan.schema_version != EXECUTION_PLAN_SCHEMA {
        problems.push(format!(
            "plan '{}': schemaVersion must be '{EXECUTION_PLAN_SCHEMA}', got '{}'",
            plan.id, plan.schema_version
        ));
    }
    if !plan.nodes.contains_key(&plan.entry_step) {
        problems.push(format!(
            "plan '{}': entryStep '{}' is not a node",
            plan.id, plan.entry_step
        ));
    }

    for (name, node) in &plan.nodes {
        if !valid_node_id(name) {
            problems.push(format!(
                "{}: node ID must contain 1 through 128 ASCII letters, digits, underscores or hyphens",
                at(name)
            ));
        }
        if let Some(policy) = &node.on_error {
            if policy.backoff_s > MAX_ON_ERROR_BACKOFF_S {
                problems.push(format!(
                    "{}: onError.backoffS must be at most {MAX_ON_ERROR_BACKOFF_S} seconds",
                    at(name)
                ));
            }
        }
        if node
            .prompt_supplement
            .as_ref()
            .is_some_and(|s| s.len() > 16 * 1024)
        {
            problems.push(format!("{}: promptSupplement exceeds 16 KiB", at(name)));
        }
        for routes in [node.routes.as_ref(), node.on_item_outcome.as_ref()]
            .into_iter()
            .flatten()
        {
            if routes.is_empty()
                || routes.len() > 16
                || routes.keys().any(|key| {
                    let mut chars = key.bytes();
                    key.len() > 64
                        || !matches!(chars.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
                        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                })
            {
                problems.push(format!("{}: routes requires 1 through 16 ASCII identifier names, at most 64 characters", at(name)));
            }
            // Whether both exit forms are legal depends on the registry's
            // completion mode; tool-or-route explicitly requires both.
            for target in routes.values() {
                if matches!(
                    target,
                    Transition::Inline(InlineTransition {
                        status: InlineStatus::Paused,
                        resume_at: None,
                        ..
                    })
                ) {
                    problems.push(format!(
                        "{}: paused route requires resumeAt continuation",
                        at(name)
                    ));
                }
                check_inline_resume(target, plan, &at(name), problems);
                if let Transition::Node(target) = target {
                    if !plan.nodes.contains_key(target) {
                        problems.push(format!(
                            "{}: route targets unknown node '{target}'",
                            at(name)
                        ));
                    }
                }
            }
        }
        if node.on_item_outcome.is_some() && node.kind != NodeKind::Foreach {
            problems.push(format!(
                "{}: onItemOutcome is only valid on foreach nodes",
                at(name)
            ));
        }
        if let (Some(routes), Some(body)) = (&node.on_item_outcome, &node.body) {
            if let Some(child) = plan.find_plan(&body.flow_ref) {
                for outcome in routes.keys() {
                    if !child
                        .nodes
                        .values()
                        .any(|n| n.kind == NodeKind::End && n.outcome.as_ref() == Some(outcome))
                    {
                        problems.push(format!(
                            "{}: onItemOutcome '{outcome}' is not a declared body end outcome",
                            at(name)
                        ));
                    }
                }
            }
        }
        if node.kind != NodeKind::Task
            && (node.routes.is_some() || node.prompt_supplement.is_some())
        {
            problems.push(format!("{}: task-only fields on other node kind", at(name)));
        }
        match node.kind {
            NodeKind::Task => {
                if node.task_ref.is_none() {
                    problems.push(format!("{}: task node missing taskRef", at(name)));
                }
                // A successful task needs either its fixed verified edge or
                // authored routes. Errors retain the onError default.
                if node.on_verified.is_none() && node.routes.is_none() {
                    problems.push(format!(
                        "{}: task node missing onVerified (unhandled-transition would occur)",
                        at(name)
                    ));
                }
                for f in foreign_foreach_fields(node) {
                    problems.push(format!(
                        "{}: '{f}' is only valid on foreach nodes",
                        at(name)
                    ));
                }
                if let Some(NodeOutputs::Task(outputs)) = &node.outputs {
                    for (output_name, output) in outputs {
                        if let NodeOutputBinding::FromArtifact {
                            view: Some(view), ..
                        } = output
                        {
                            if let Err(error) = view.validate() {
                                problems
                                    .push(format!("{}: output '{output_name}': {error}", at(name)));
                            }
                        }
                    }
                }
                if let Some(o) = &node.outputs {
                    if !matches!(o, NodeOutputs::Task(_)) {
                        problems.push(format!(
                            "{}: task node 'outputs' must use fromArtifact/from bindings",
                            at(name)
                        ));
                    }
                }
                if node.outcome.is_some() {
                    problems.push(format!(
                        "{}: 'outcome' is only valid on end nodes",
                        at(name)
                    ));
                }
            }
            NodeKind::Foreach => {
                for (field, present) in [
                    ("items", node.items.is_some()),
                    ("itemKey", node.item_key.is_some()),
                    ("bindAs", node.bind_as.is_some()),
                    ("enumeration", node.enumeration.is_some()),
                    ("ordering", node.ordering.is_some()),
                    ("body", node.body.is_some()),
                ] {
                    if !present {
                        problems.push(format!("{}: foreach node missing '{field}'", at(name)));
                    }
                }
                // enumeration/ordering values themselves are closed by serde
                // (snapshot / source only).
                if node.on_complete.is_none() {
                    problems.push(format!("{}: foreach node missing onComplete", at(name)));
                }
                if node.on_empty.is_none() {
                    problems.push(format!(
                        "{}: foreach node missing onEmpty (snapshot length 0 is always possible)",
                        at(name)
                    ));
                }
                if node.max_items.is_some() && node.on_limit.is_none() {
                    problems.push(format!(
                        "{}: foreach declares maxItems but no onLimit",
                        at(name)
                    ));
                }
                if let Some(body) = &node.body {
                    if plan.find_plan(&body.flow_ref).is_none() {
                        problems.push(format!(
                            "{}: body flowRef '{}' is not loaded (expected sibling file '{}.flow.yaml')",
                            at(name),
                            body.flow_ref,
                            body.flow_ref
                        ));
                    }
                }
                if let Some(between) = &node.between {
                    if plan.find_plan(&between.flow_ref).is_none() {
                        problems.push(format!(
                            "{}: between flowRef '{}' is not loaded",
                            at(name),
                            between.flow_ref
                        ));
                    }
                }
                if node.task_ref.is_some()
                    || node.on_verified.is_some()
                    || node.on_error.is_some()
                    || node.verify.is_some()
                {
                    problems.push(format!("{}: task-only field(s) on foreach node", at(name)));
                }
                match &node.outputs {
                    None => problems.push(format!(
                        "{}: foreach node missing 'outputs' aggregation (§4.6)",
                        at(name)
                    )),
                    Some(NodeOutputs::Foreach(spec)) => {
                        if spec.values().any(|collect| collect.view.is_some()) && spec.len() > 8 {
                            problems.push(format!(
                                "{}: collection views permit at most 8 named collections",
                                at(name)
                            ));
                        }
                        for collect in spec.values() {
                            if let Some(view) = &collect.view {
                                if let Err(error) = view.validate() {
                                    problems.push(format!("{}: {error}", at(name)));
                                }
                            }
                        }
                    }
                    Some(NodeOutputs::Task(_)) => problems.push(format!(
                        "{}: foreach 'outputs' must be collect specs (§4.6), not task bindings",
                        at(name)
                    )),
                }
                if node.outcome.is_some() {
                    problems.push(format!(
                        "{}: 'outcome' is only valid on end nodes",
                        at(name)
                    ));
                }
            }
            NodeKind::End => {
                match &node.outcome {
                    None => problems.push(format!("{}: end node missing outcome", at(name))),
                    Some(o) if o.is_empty() => {
                        problems.push(format!("{}: end node has empty outcome", at(name)))
                    }
                    _ => {}
                }
                if node.task_ref.is_some()
                    || node.items.is_some()
                    || node.body.is_some()
                    || node.on_verified.is_some()
                    || node.on_complete.is_some()
                    || node.on_empty.is_some()
                    || node.on_error.is_some()
                    || node.on_limit.is_some()
                    || node.outputs.is_some()
                    || node.verify.is_some()
                {
                    problems.push(format!(
                        "{}: end node carries fields of other kinds",
                        at(name)
                    ));
                }
            }
        }

        // Transition targets must exist (§4.2).
        let transitions: Vec<(&str, &Transition)> = [
            node.on_verified.as_ref().map(|t| ("onVerified", t)),
            node.on_complete.as_ref().map(|t| ("onComplete", t)),
            node.on_empty.as_ref().map(|t| ("onEmpty", t)),
            node.on_limit.as_ref().map(|t| ("onLimit", t)),
            node.on_error.as_ref().map(|e| ("onError.then", &e.then)),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (key, t) in transitions {
            check_inline_resume(t, plan, &at(name), problems);
            if let Transition::Node(target) = t {
                if !plan.nodes.contains_key(target) {
                    problems.push(format!(
                        "{}: transition '{key}' targets unknown node '{target}'",
                        at(name)
                    ));
                }
            }
        }
    }
}

fn check_inline_resume(t: &Transition, plan: &ExecutionPlan, at: &str, problems: &mut Vec<String>) {
    if let Transition::Inline(inline) = t {
        if let Some(target) = &inline.resume_at {
            if inline.status != InlineStatus::Paused {
                problems.push(format!(
                    "{at}: resumeAt is only valid on paused transitions"
                ));
            }
            if !plan.nodes.contains_key(target) {
                problems.push(format!("{at}: resumeAt targets unknown node '{target}'"));
            }
        }
    }
}

fn foreign_foreach_fields(node: &Node) -> Vec<&'static str> {
    let mut out = Vec::new();
    if node.items.is_some() {
        out.push("items");
    }
    if node.item_key.is_some() {
        out.push("itemKey");
    }
    if node.bind_as.is_some() {
        out.push("bindAs");
    }
    if node.enumeration.is_some() {
        out.push("enumeration");
    }
    if node.ordering.is_some() {
        out.push("ordering");
    }
    if node.max_items.is_some() {
        out.push("maxItems");
    }
    if node.on_limit.is_some() {
        out.push("onLimit");
    }
    if node.body.is_some() {
        out.push("body");
    }
    if node.between.is_some() {
        out.push("between");
    }
    if node.on_empty.is_some() {
        out.push("onEmpty");
    }
    if node.on_item_outcome.is_some() {
        out.push("onItemOutcome");
    }
    if node.on_complete.is_some() {
        out.push("onComplete");
    }
    out
}

/// Combined plan+registry checks (Executor::new): taskRefs resolve, slots
/// exist, prompt files are readable, tool allowlist entries are `server.tool`.
pub fn validate_with_registry(
    plan: &ExecutionPlan,
    registry: &TaskRegistry,
) -> Result<(), ValidationError> {
    let mut problems = Vec::new();
    if let Err(error) = registry.validate_completion() {
        problems.push(error.to_string());
    }
    if registry.schema_version != crate::registry::TASK_REGISTRY_SCHEMA {
        problems.push(format!(
            "registry schemaVersion must be '{}', got '{}'",
            crate::registry::TASK_REGISTRY_SCHEMA,
            registry.schema_version
        ));
    }
    fn visit(plan: &ExecutionPlan, registry: &TaskRegistry, problems: &mut Vec<String>) {
        check_plan_against_registry(plan, registry, problems);
        for sub in plan.subflows.values() {
            visit(sub, registry, problems);
        }
    }
    visit(plan, registry, &mut problems);
    if let Err(error) =
        validate_task_result_artifacts(plan, registry, |_, _| trollm_mcp::policy::ToolClass::Read)
    {
        problems.push(error.to_string());
    }
    if let Err(error) =
        validate_mutation_safety(plan, registry, |_, _| trollm_mcp::policy::ToolClass::Read)
    {
        problems.push(error.to_string());
    }
    if !problems.is_empty() {
        return Err(ValidationError::new(
            format!("plan '{}' + registry", plan.id),
            problems,
        ));
    }
    Ok(())
}

fn check_plan_against_registry(
    plan: &ExecutionPlan,
    registry: &TaskRegistry,
    problems: &mut Vec<String>,
) {
    for (name, node) in &plan.nodes {
        if node.kind != NodeKind::Task {
            continue;
        }
        let Some(task_ref) = &node.task_ref else {
            continue;
        };
        let Some(task) = registry.task(task_ref) else {
            problems.push(format!(
                "plan '{}' node '{name}': taskRef '{task_ref}' not in registry",
                plan.id
            ));
            continue;
        };
        if let Some(NodeOutputs::Task(outputs)) = &node.outputs {
            for (output, binding) in outputs {
                if matches!(binding, NodeOutputBinding::FromModel { .. })
                    && matches!(
                        task.output_contract.outputs.get(output),
                        Some(crate::registry::OutputSpec::FromArtifact {
                            schema: Some(_),
                            ..
                        })
                    )
                {
                    problems.push(format!("plan '{}' node '{name}': schema-bearing artifact output '{output}' cannot be overridden by model output", plan.id));
                }
                if matches!(binding, NodeOutputBinding::FromArtifact { .. })
                    && matches!(
                        task.output_contract.outputs.get(output),
                        Some(crate::registry::OutputSpec::FromModel {
                            schema: Some(_),
                            ..
                        })
                    )
                {
                    problems.push(format!("plan '{}' node '{name}': schema-bearing model output '{output}' cannot be overridden by artifact output", plan.id));
                }
            }
        }
        if matches!(
            task.completion,
            crate::registry::TaskCompletion::FlowRoute
                | crate::registry::TaskCompletion::ToolResult
                | crate::registry::TaskCompletion::ToolOrRoute
        ) && matches!(&node.outputs, Some(NodeOutputs::Task(outputs)) if outputs.values().any(|o| matches!(o, NodeOutputBinding::FromModel { .. })))
        {
            problems.push(format!("plan '{}' node '{name}': tool-result, flow-route and tool-or-route completion cannot bind model outputs", plan.id));
        }
        let exits_valid = match task.completion {
            crate::registry::TaskCompletion::ToolOrRoute => {
                node.routes.is_some() && node.on_verified.is_some()
            }
            crate::registry::TaskCompletion::FlowRoute => {
                node.routes.is_some() && node.on_verified.is_none()
            }
            _ => node.routes.is_none() && node.on_verified.is_some(),
        };
        if !exits_valid {
            problems.push(format!("plan '{}' node '{name}': flow-route requires routes without onVerified; tool-or-route requires both; other completions require onVerified without routes", plan.id));
        }
        if let Some(model) = task
            .model_slot()
            .filter(|model| !plan.models.contains_key(*model))
        {
            problems.push(format!(
                "plan '{}' node '{name}': task '{task_ref}' uses model slot '{}' not declared in plan.models",
                plan.id, model
            ));
        }
        if task.execution == crate::registry::TaskExecution::Tool
            && (node.routes.is_some() || node.prompt_supplement.is_some())
        {
            problems.push(format!(
                "plan '{}' node '{name}': tool execution forbids routes and promptSupplement",
                plan.id
            ));
        }
        for tool in &task.tools {
            if tool.split('.').count() != 2 {
                problems.push(format!(
                    "task '{task_ref}': tool allowlist entry '{tool}' is not '<server>.<tool>'"
                ));
            }
        }
        if task.execution == crate::registry::TaskExecution::Model {
            if let Err(e) = registry.prompt_template(task) {
                problems.push(format!("task '{task_ref}': {e}"));
            }
        }
    }
}

/// Root storage policies govern every child and cannot be downgraded by adapters.
pub(crate) fn validate_task_result_artifacts(
    root: &ExecutionPlan,
    registry: &TaskRegistry,
    class: impl Fn(&str, &str) -> trollm_mcp::policy::ToolClass,
) -> Result<(), ValidationError> {
    fn visit(
        plan: &ExecutionPlan,
        root: &ExecutionPlan,
        registry: &TaskRegistry,
        class: &impl Fn(&str, &str) -> trollm_mcp::policy::ToolClass,
        owners: &mut std::collections::BTreeMap<String, String>,
        problems: &mut Vec<String>,
    ) {
        let policies = serde_yaml::to_value(&root.mcp.policies)
            .map_err(anyhow::Error::from)
            .and_then(|v| trollm_mcp::PolicySet::from_yaml_value(&v));
        for (id, node) in &plan.nodes {
            let Some(task) = node.task_ref.as_deref().and_then(|n| registry.task(n)) else {
                continue;
            };
            let Some(name) = &task.result_artifact else {
                continue;
            };
            let Some(tool) = task.tools.first() else {
                continue;
            };
            let Some((server, method)) = tool.split_once('.') else {
                continue;
            };
            let result = policies
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .and_then(|p| p.task_result_policy(server, method, name));
            if let Err(e) = result {
                problems.push(format!("plan '{}' node '{id}': {e:#}", plan.id));
            }
            if class(server, method) != trollm_mcp::policy::ToolClass::Read {
                problems.push(format!(
                    "task resultArtifact '{name}' requires actual gateway read classification"
                ));
            }
            if owners
                .insert(name.clone(), tool.clone())
                .is_some_and(|old| old != *tool)
            {
                problems.push(format!(
                    "task resultArtifact '{name}' is owned by unrelated tools"
                ));
            }
            let valid_binding = task.output_contract.outputs.iter().any(|(output, spec)| {
                if let Some(NodeOutputs::Task(overrides)) = &node.outputs {
                    if let Some(value) = overrides.get(output) {
                        return matches!(value, NodeOutputBinding::FromArtifact { from_artifact, .. } if from_artifact == name);
                    }
                }
                matches!(spec, crate::registry::OutputSpec::FromArtifact { from_artifact, .. } if from_artifact == name)
            });
            if !valid_binding
                || !task
                    .output_contract
                    .require_artifact_complete
                    .contains(name)
            {
                problems.push(format!("task resultArtifact '{name}' requires a matching artifact output and completeness requirement"));
            }
        }
        for sub in plan.subflows.values() {
            visit(sub, root, registry, class, owners, problems);
        }
    }
    let mut problems = Vec::new();
    visit(
        root,
        root,
        registry,
        &class,
        &mut std::collections::BTreeMap::new(),
        &mut problems,
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new("task resultArtifact", problems))
    }
}

/// Root policies govern the shared run gateway, including nested subflows.
/// Also consult the actual gateway so composition overrides cannot downgrade
/// mutation safety. Intent-log tasks conservatively disallow automatic retry,
/// including those whose current grants are read-only.
pub(crate) fn validate_mutation_safety(
    root: &ExecutionPlan,
    registry: &TaskRegistry,
    class: impl Fn(&str, &str) -> trollm_mcp::policy::ToolClass,
) -> Result<(), ValidationError> {
    fn visit(
        plan: &ExecutionPlan,
        root: &ExecutionPlan,
        registry: &TaskRegistry,
        class: &impl Fn(&str, &str) -> trollm_mcp::policy::ToolClass,
        problems: &mut Vec<String>,
    ) {
        for (name, node) in &plan.nodes {
            if node.kind != NodeKind::Task {
                continue;
            }
            let Some(task) = node.task_ref.as_deref().and_then(|id| registry.task(id)) else {
                continue;
            };
            let journaled = task
                .mutations
                .as_ref()
                .is_some_and(|m| m.idempotency == crate::registry::Idempotency::IntentLog);
            let mutation = task.tools.iter().any(|tool| {
                let Some((server, tool_name)) = tool.split_once('.') else {
                    return false;
                };
                if server == "flow" {
                    return false;
                }
                root.mcp
                    .policies
                    .get(tool)
                    .and_then(|p| p.get("class"))
                    .and_then(|c| c.as_str())
                    == Some("mutation")
                    || class(server, tool_name) == trollm_mcp::policy::ToolClass::Mutation
            });
            if mutation && !journaled {
                problems.push(format!("plan '{}' node '{name}': task granted mutation tools requires mutations.idempotency=intent-log", plan.id));
            }
            if (mutation || journaled) && node.on_error.as_ref().is_some_and(|p| p.retry > 0) {
                problems.push(format!("plan '{}' node '{name}': mutation or intent-log task cannot use automatic onError.retry", plan.id));
            }
        }
        for child in plan.subflows.values() {
            visit(child, root, registry, class, problems);
        }
    }
    let mut problems = Vec::new();
    visit(root, root, registry, &class, &mut problems);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new(
            format!("plan '{}' mutation safety", root.id),
            problems,
        ))
    }
}

/// New authoring/import provenance check. Keep this separate from runtime
/// validation: installed v1 plans may retain historical string identifiers.
/// Provenance is optional metadata, never a content pin or trust anchor.
pub fn validate_authoring_provenance(
    plan: &ExecutionPlan,
    registry: &TaskRegistry,
) -> Result<(), ValidationError> {
    fn check(source: &Option<SourceBaseline>, at: String, problems: &mut Vec<String>) {
        if let Some(source) = source {
            if source.sha256.len() != 64
                || !source
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                problems.push(format!(
                    "{at}: sourceBaseline.sha256 must be 64 lowercase hexadecimal characters"
                ));
            }
        }
    }
    fn visit(plan: &ExecutionPlan, problems: &mut Vec<String>) {
        check(
            &plan.source_baseline,
            format!("plan '{}'", plan.id),
            problems,
        );
        for child in plan.subflows.values() {
            visit(child, problems);
        }
    }
    let mut problems = Vec::new();
    visit(plan, &mut problems);
    check(
        &registry.source_baseline,
        "task registry".into(),
        &mut problems,
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new("new authoring provenance", problems))
    }
}
