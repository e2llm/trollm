use crate::inspect::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRef {
    pub flow: String,
    pub node: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Presentation {
    pub format: String,
    pub title: String,
    pub purpose: String,
    #[serde(default, deserialize_with = "optional_text")]
    pub inputs_description: Option<String>,
    #[serde(default, deserialize_with = "optional_text")]
    pub result_description: Option<String>,
    pub phases: Vec<Phase>,
}
// Missing descriptions are optional; explicit null is outside the JSON schema.
fn optional_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub id: String,
    pub title: String,
    pub description: String,
    pub nodes: Vec<NodeRef>,
}

pub fn check(
    text: Option<&String>,
    nodes: &BTreeSet<NodeRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Option<serde_json::Value>, Vec<NodeRef>) {
    let Some(text) = text else {
        diagnostics.push(Diagnostic::warning(
            "presentation-missing",
            "No authored phase descriptions",
            "flow-review.json",
        ));
        return (None, nodes.iter().cloned().collect());
    };
    let raw = serde_json::from_str(text).ok();
    let Ok(p) = serde_json::from_str::<Presentation>(text) else {
        diagnostics.push(Diagnostic::warning(
            "presentation-invalid",
            "Presentation does not match the supported format",
            "flow-review.json",
        ));
        return (None, nodes.iter().cloned().collect());
    };
    if p.phases.len() > 1024
        || p.phases
            .iter()
            .map(|phase| phase.nodes.len())
            .sum::<usize>()
            > 10000
    {
        diagnostics.push(Diagnostic::warning(
            "presentation-invalid",
            "Presentation exceeds 1024 phases or 10000 total references",
            "flow-review.json",
        ));
        return (None, nodes.iter().cloned().collect());
    }
    let nonempty = |s: &str| !s.trim().is_empty() && s.len() <= 16384;
    let mut assigned = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut valid = p.format == "trollm.flow-presentation.v1"
        && nonempty(&p.title)
        && nonempty(&p.purpose)
        && p.phases.len() <= 1024;
    valid &= p.inputs_description.as_ref().is_none_or(|s| nonempty(s))
        && p.result_description.as_ref().is_none_or(|s| nonempty(s));
    let mut references = 0usize;
    for phase in &p.phases {
        references += phase.nodes.len();
        valid &= references <= 10000;
        valid &= nonempty(&phase.id)
            && nonempty(&phase.title)
            && nonempty(&phase.description)
            && ids.insert(&phase.id)
            && phase.nodes.len() <= 10000;
        for node in &phase.nodes {
            valid &= nonempty(&node.flow) && nonempty(&node.node);
            if !nodes.contains(node) {
                valid = false;
                diagnostics.push(Diagnostic::warning(
                    "presentation-unknown-node",
                    "Phase references an unknown flow/node",
                    "flow-review.json",
                ));
            } else if !assigned.insert(node.clone()) {
                valid = false;
                diagnostics.push(Diagnostic::warning(
                    "presentation-duplicate-node",
                    "Node is assigned to more than one phase",
                    "flow-review.json",
                ));
            }
        }
    }
    if !valid {
        diagnostics.push(Diagnostic::warning(
            "presentation-invalid",
            "Presentation requires bounded nonempty descriptions and unique phase IDs",
            "flow-review.json",
        ));
        return (None, nodes.iter().cloned().collect());
    }
    (raw, nodes.difference(&assigned).cloned().collect())
}
