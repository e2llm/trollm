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
        match node.kind {
            NodeKind::Task => {
                if node.task_ref.is_none() {
                    problems.push(format!("{}: task node missing taskRef", at(name)));
                }
                // Transition coverage (§4.2): a task always ends verified or in
                // error; error is covered by the onError default, so onVerified
                // is statically required.
                if node.on_verified.is_none() {
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
                    Some(NodeOutputs::Foreach(_)) => {}
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
    check_plan_against_registry(plan, registry, &mut problems);
    for sub in plan.subflows.values() {
        check_plan_against_registry(sub, registry, &mut problems);
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
        if !plan.models.contains_key(&task.model) {
            problems.push(format!(
                "plan '{}' node '{name}': task '{task_ref}' uses model slot '{}' not declared in plan.models",
                plan.id, task.model
            ));
        }
        for tool in &task.tools {
            if tool.split('.').count() != 2 {
                problems.push(format!(
                    "task '{task_ref}': tool allowlist entry '{tool}' is not '<server>.<tool>'"
                ));
            }
        }
        if let Err(e) = registry.prompt_template(task) {
            problems.push(format!("task '{task_ref}': {e}"));
        }
    }
}
