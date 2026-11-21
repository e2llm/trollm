//! Target-owned executable member offer. This compiles existing native formats;
//! it does not publish/admit a source, resolve ownership or establish competence.
use crate::collection::{raw_sha256, SelectionRecord};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use trollm_mcp::preparation::js_sorted_json_bytes;

pub const PROFILE: &str = "trollm.native-collection-members.v1-proposal";
pub const SOURCE_SCHEMA: &str = "moray.declared-native-collection.v1-proposal";
pub const ROOT: &str = "selected-collection";
pub const BODY: &str = "selected-item";
pub const BETWEEN: &str = "selected-between";
pub const N1_OFFER_PROFILE: &str = "trollm.native-collection-members.v2-proposal";
pub const WORKLOAD_600_PROFILE: &str = "trollm.native-collection-members.v4-proposal";
pub const WORKLOAD_CLOCK_STARTUP_ATTEMPTS: u64 = 2;
pub const WORKLOAD_CLOCK_PREPARATION_CONTEXTS: usize = 11;
pub const WORKLOAD_CLOCK_ATTEMPTS_PER_CONTEXT: u64 = 12;
pub const WORKLOAD_CLOCK_MAINTENANCE_ATTEMPTS: u64 = 49;
pub const WORKLOAD_CLOCK_TOTAL_ATTEMPTS: u64 = WORKLOAD_CLOCK_STARTUP_ATTEMPTS
    + WORKLOAD_CLOCK_PREPARATION_CONTEXTS as u64 * WORKLOAD_CLOCK_ATTEMPTS_PER_CONTEXT
    + WORKLOAD_CLOCK_MAINTENANCE_ATTEMPTS;

pub fn is_matched_profile(profile: &str) -> bool {
    [MATCHED_PROFILE, WORKLOAD_600_PROFILE].contains(&profile)
}
pub const MATCHED_PROFILE: &str = "trollm.native-collection-members.v3-proposal";
pub const MATCHED_READY: &str = "controlled-collection-selected-ready.v3";
pub const WORKLOAD_600_NATIVE_PROFILE: &str = "controlled-collection-native-selected.v4-proposal";
pub const MATCHED_NATIVE_PROFILE: &str = "controlled-collection-native-selected.v3-proposal";
pub const MATCHED_GUARD: &str = "controlled-collection-selected-guard.v3";

/// Collection source union, separate from text admission. Producer supplies real
/// fixture/version identity; raw member inspection does not establish current
/// authority, runtime support or recorded execution lineage.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionSource {
    pub schema_version: String,
    pub origin: String,
    pub owner: crate::collection::Owner,
    pub fixture_id: String,
    pub version_id: String,
    pub selection_ref: crate::collection::OwnedRef,
    pub selection_raw_sha256: String,
    pub profile_id: String,
    pub profile_sha256: String,
    pub installation_sha256: String,
    pub qualification: String,
}
impl CollectionSource {
    /// Source shape/member binding only. Current server authority and genuine
    /// invocation/worker admission are required separately before any execution.
    pub fn inspect(&self, directory: &Path, endpoint: &str) -> Result<InspectedCollectionMembers> {
        trollm_mcp::ArtifactSchema::compile(&serde_json::from_str(include_str!(
            "../../../schemas/declared-native-collection-source.v1.schema.json"
        ))?)?
        .validate(&serde_json::to_value(self)?, 65536)?;
        ensure!(
            self.schema_version == SOURCE_SCHEMA
                && self.origin == "DECLARED"
                && [PROFILE, MATCHED_PROFILE, WORKLOAD_600_PROFILE]
                    .contains(&self.profile_id.as_str())
                && self.qualification == "unqualified",
            "unsupported collection source profile"
        );
        ensure!(
            [&self.fixture_id, &self.version_id]
                .iter()
                .all(|id| !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')),
            "collection source identity unavailable"
        );
        let raw = crate::setup::read_input_evidence(directory, "selection.json", 65536)?;
        ensure!(
            raw_sha256(&raw) == self.selection_raw_sha256
                && self.selection_ref.sha256 == self.selection_raw_sha256,
            "collection source raw selection drift"
        );
        let selection = SelectionRecord::decode(&raw, &self.selection_ref, &self.owner)?;
        let context = crate::setup::read_input_evidence(directory, "context.txt", 2048)?;
        ensure!(
            raw_sha256(&crate::setup::read_input_evidence(
                directory,
                "manifest.json",
                1024 * 1024
            )?) == self.installation_sha256,
            "collection source installation drift"
        );
        ensure!(
            raw_sha256(&crate::setup::read_input_evidence(
                directory,
                "collection-profile.json",
                65536
            )?) == self.profile_sha256,
            "collection source profile drift"
        );
        if self.profile_id == WORKLOAD_600_PROFILE {
            CollectionMembers::inspect_matched_600(directory, &selection, &context, endpoint)
        } else if self.profile_id == MATCHED_PROFILE {
            CollectionMembers::inspect_matched(directory, &selection, &context, endpoint)
        } else {
            CollectionMembers::inspect(directory, &selection, &context, endpoint)
        }
    }
}

/// These bytes are the offer, not a parallel DSL. JSON members named *.yaml
/// are valid YAML consumed by the existing native loaders without translation.
pub struct CollectionMembers {
    pub files: BTreeMap<String, Vec<u8>>,
}
pub struct InspectedCollectionMembers {
    pub member_profile: String,
    pub root_plan_pin: String,
    pub bindings: Vec<crate::collection_provider::ActionBinding>,
    pub expected_snapshot: Vec<Value>,
    pub selection_input_json: String,
}
fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut raw = serde_json::to_vec(value)?;
    raw.push(b'\n');
    Ok(raw)
}
fn plan(id: &str, endpoint: &str, nodes: Value, entry: &str) -> Value {
    json!({"schemaVersion":"ledger.execution-plan.v1","id":id,"version":1,
        "input":{"promptTemplate":"{{ user_input }}"},
        "limits":{"maxSteps":32,"maxReasonerCalls":0,"maxTokensTotal":32768,"timeoutS":300,"artifactMaxMb":8},
        "onLimit":"abort","models":{"coder":{"ctx":4096}},
        "mcp":{"servers":[{"name":"e2llm","url":endpoint}],"policies":{
            "local.prepare_worklist":{"class":"read","resultArtifact":"worklist","resultValidation":{"maxBytes":49152,"schema":{"type":"object"}}},
            "e2llm.act":{"class":"mutation","passthrough":true,"timeoutS":30,"resultArtifact":"act","resultValidation":{"maxBytes":65536,"schema":{"type":"object"}}}}},
        "entryStep":entry,"nodes":nodes})
}
impl CollectionMembers {
    pub fn compile(selection: &SelectionRecord, context: &[u8], endpoint: &str) -> Result<Self> {
        // Re-decode original immutable input; never trust a mutated projection.
        let original = SelectionRecord::decode(
            &selection.original_bytes,
            &selection.selection_ref,
            &selection.selection.owner,
        )?;
        ensure!(
            serde_json::to_value(&original.selection)?
                == serde_json::to_value(&selection.selection)?,
            "selection projection drift"
        );
        ensure!(
            selection.selection.items.len() <= 3,
            "selected target roster exceeds three items"
        );
        match selection.selection.variant {
            crate::collection::Variant::Original | crate::collection::Variant::Changed => ensure!(
                selection
                    .selection
                    .items
                    .iter()
                    .enumerate()
                    .all(|(index, item)| item.input_position as usize == index),
                "complete selected source must preserve its own request positions"
            ),
            crate::collection::Variant::Subset => ensure!(
                selection.selection.items.len() < 3
                    && selection
                        .selection
                        .items
                        .iter()
                        .all(|item| item.input_position < 3)
                    && selection
                        .selection
                        .items
                        .windows(2)
                        .all(|pair| pair[0].input_position < pair[1].input_position),
                "selected subset must be ordered and strictly smaller than the bounded domain"
            ),
            crate::collection::Variant::Empty => {}
        }

        ensure!(
            matches!(
                endpoint,
                "https://api.e2llm.dev/mcp" | "https://mcp.e2llm.dev/mcp"
            ),
            "selected target endpoint unsupported"
        );
        ensure!(
            context.len() <= 2048 && raw_sha256(context) == selection.selection.context_ref.sha256,
            "selected context raw identity/size mismatch"
        );
        let context_text = std::str::from_utf8(context).context("selected context must be UTF8")?;
        let prefix = selection.selection.continuation.prefix_length as usize;
        let items = &selection.selection.items[prefix..];
        let association = js_sorted_json_bytes(&json!({"selectionRef":selection.selection_ref,
            "selectionRawSha256":raw_sha256(&selection.original_bytes),"selectionId":selection.selection.selection_id,
            "caseId":selection.selection.case_id,"invocationKey":selection.selection.invocation_key,
            "knownPrefixLength":prefix,"contextRef":selection.selection.context_ref,"context":context_text}))?;
        ensure!(
            association.len() <= 4096,
            "worklist context exceeds existing tool bound"
        );
        let worklist = js_sorted_json_bytes(
            &json!({"context":String::from_utf8(association)?,"items":items}),
        )?;
        ensure!(
            worklist.len() <= 49152,
            "worklist exceeds existing tool bound"
        );
        if !items.is_empty() {
            // Exercise the actual deterministic tool boundary, including its
            // native id/value wrapper, without executing any model or effect.
            trollm_mcp::local::prepare_worklist(&json!({"input":std::str::from_utf8(&worklist)?}))?;
        }
        let setup_model: Value = serde_json::from_str(include_str!(
            "../../../flows/starters/browser-summary/setup.json"
        ))?;
        ensure!(
            setup_model["models"]["coder"]["sha256"]
                == "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517"
                && setup_model["models"]["coder"]["ctx"] == 4096,
            "selected coder profile changed"
        );
        let setup = json!({"format":"trollm.flow-setup.v1","title":"Selected native collection",
            "input":{"fields":[{"key":"selectionSha256","label":"Canonical admitted selection SHA256","kind":"text","required":true,"maxLength":64}],
                "toolArguments":{"local.prepare_worklist":{"fixed":{"input":std::str::from_utf8(&worklist)?}}}},
            "models":{"coder":setup_model["models"]["coder"]},
            "connections":{"e2llm":{"url":endpoint,"auth":"bearer"}}});
        let mut files = BTreeMap::new();
        files.insert("selection.json".into(), selection.original_bytes.clone());
        files.insert("context.txt".into(), context.to_vec());
        files.insert("prepared-worklist.json".into(), worklist.clone());
        files.insert("setup.json".into(), encode(&setup)?);
        let mut tasks = serde_json::Map::new();
        let mut root = if items.is_empty() {
            plan(
                ROOT,
                endpoint,
                json!({"done":{"kind":"end","outcome":"succeeded"}}),
                "done",
            )
        } else {
            tasks.insert("prepare-worklist".into(), json!({"model":"coder","prompt":"prompts/prepare.md","tools":["local.prepare_worklist"],
                "completion":"tool-result","loop":{"maxSteps":1},
                "toolArguments":{"local.prepare_worklist":{"fixed":{"input":std::str::from_utf8(&worklist)?},"modelFields":[]}},
                "outputContract":{"outputs":{"prepared":{"fromArtifact":"worklist","format":"json","type":"object"}},"requireArtifactComplete":["worklist"]}}));
            files.insert("prompts/prepare.md".into(), b"Call local.prepare_worklist exactly once with no model-owned arguments. Do not claim a browser outcome.\n".to_vec());
            for (name, purpose, action) in [
                ("candidate", "candidate-select", "click"),
                ("quantity", "quantity-editor", "type"),
                ("add", "cart-add", "click"),
            ] {
                let mut rule = json!({"fixed":{"action":action},"modelFields":[]});
                if action == "type" {
                    rule["fromVars"] = json!({"value":"item.quantityText"});
                }
                tasks.insert(name.into(), json!({"model":"coder","prompt":format!("prompts/{name}.md"),"tools":["e2llm.act"],
                    "completion":"tool-result","loop":{"maxSteps":1},"mutations":{"idempotency":"intent-log","onUncertain":"fail-run"},
                    "toolArguments":{"e2llm.act":rule}}));
                files.insert(format!("prompts/{name}.md"), format!("Request the {purpose} action once using e2llm.act with no model-owned arguments. Native preparation supplies current browser/session/target; never invent them. A tool response is not a verified cart outcome.\n").into_bytes());
            }
            let body = plan(
                BODY,
                endpoint,
                json!({
                "candidate":{"kind":"task","taskRef":"candidate","onVerified":"quantity"},
                "quantity":{"kind":"task","taskRef":"quantity","onVerified":"add"},
                "add":{"kind":"task","taskRef":"add","onVerified":"done"},
                "done":{"kind":"end","outcome":"succeeded"}}),
                "candidate",
            );
            files.insert(format!("{BODY}.flow.yaml"), encode(&body)?);
            plan(
                ROOT,
                endpoint,
                json!({
                "prepare":{"kind":"task","taskRef":"prepare-worklist","onVerified":"items"},
                "items":{"kind":"foreach","items":{"from":"prepare.outputs.prepared.items"},"itemKey":"id","bindAs":"selected",
                    "enumeration":"snapshot","ordering":"source","body":{"flowRef":BODY,"inputs":{"item":{"from":"selected.value"},"context":{"from":"prepare.outputs.prepared.context"}}},
                    "outputs":{"results":{"collect":"body.outputs","indexBy":"itemKey"}},"onEmpty":"done","onComplete":"done"},
                "done":{"kind":"end","outcome":"succeeded"}}),
                "prepare",
            )
        };
        root["setup"] = json!({"path":"setup.json","sha256":raw_sha256(&files["setup.json"])});
        files.insert(format!("{ROOT}.flow.yaml"), encode(&root)?);
        files.insert(format!("{ROOT}.tasks.yaml"), encode(&json!({"schemaVersion":"ledger.task-registry.v1","id":ROOT,"version":1,"tasks":tasks}))?);
        files.insert("collection-profile.json".into(), encode(&json!({"schema":PROFILE,"sourceSchema":SOURCE_SCHEMA,
            "selectionRef":selection.selection_ref,"selectionRawSha256":raw_sha256(&selection.original_bytes),
            "knownPrefixLength":prefix,"suffixLength":items.len(),"worklistSha256":raw_sha256(&worklist),
            "rootFlow":format!("{ROOT}.flow.yaml"),"taskRegistry":format!("{ROOT}.tasks.yaml"),
            "cursorNode":if items.is_empty() { Value::Null } else { json!("items") },
            "actions":if items.is_empty() { json!([]) } else { json!([
                {"flow":BODY,"node":"candidate","taskRef":"candidate","purpose":"candidate-select"},
                {"flow":BODY,"node":"quantity","taskRef":"quantity","purpose":"quantity-editor"},
                {"flow":BODY,"node":"add","taskRef":"add","purpose":"cart-add"}]) },
            "runtimeAdmissionSupported":false,"navigationSupported":false,
            "limits":["installed authority/clock/worker integration required","mandatory private preparation ownership required","multiple suffix items require pending-before-send navigation"]}))?);
        let manifest = json!({"format":crate::bundle::BUNDLE_FORMAT,"entryFlow":format!("{ROOT}.flow.yaml"),"taskRegistry":format!("{ROOT}.tasks.yaml"),
            "files":files.iter().map(|(path, raw)| json!({"path":path,"sha256":raw_sha256(raw)})).collect::<Vec<_>>()});
        files.insert(crate::bundle::MANIFEST_NAME.into(), encode(&manifest)?);
        Ok(Self { files })
    }

    /// Historical unadmitted N-1 offer. The matched installed path uses the
    /// distinct v3 profile; v1/v2 bytes remain unchanged.
    pub fn compile_n1_offer(
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<Self> {
        let mut members = Self::compile(selection, context, endpoint)?;
        let prefix = selection.selection.continuation.prefix_length as usize;
        let suffix = selection.selection.items.len() - prefix;
        if suffix != 0 {
            let mut root: Value =
                serde_json::from_slice(&members.files[&format!("{ROOT}.flow.yaml")])?;
            root["nodes"]["items"]["between"] = json!({"flowRef":BETWEEN});
            members
                .files
                .insert(format!("{ROOT}.flow.yaml"), encode(&root)?);
            let mut registry: Value =
                serde_json::from_slice(&members.files[&format!("{ROOT}.tasks.yaml")])?;
            registry["tasks"]["choose-continuation"] = json!({"model":"coder",
                "prompt":"prompts/choose-continuation.md", "tools":["flow.choose"],
                "completion":"flow-route", "loop":{"maxSteps":1},
                "maxOutputTokens":128});
            registry["tasks"]["navigate-next"] = json!({"model":"coder",
                "prompt":"prompts/navigate-next.md", "tools":["e2llm.act"],
                "completion":"tool-result", "loop":{"maxSteps":1},
                "mutations":{"idempotency":"intent-log","onUncertain":"fail-run"},
                "toolArguments":{"e2llm.act":{"fixed":{"action":"navigate"},"modelFields":[]}}});
            members
                .files
                .insert(format!("{ROOT}.tasks.yaml"), encode(&registry)?);
            members.files.insert("prompts/choose-continuation.md".into(),
                b"Completed suffix count: {{ items.cursor }}. Immutable suffix length: {{ items.snapshot_len }}. Call flow.choose once with next only when the completed count is strictly less than the length; otherwise choose final. This model route is not a verifier or permission to navigate. Native preparation independently refuses last-item or wrong-page effects.\n".to_vec());
            members.files.insert("prompts/navigate-next.md".into(),
                b"Request e2llm.act once with no model-owned arguments. Native preparation must arm the existing observer before the sole send and supply the exact source-computed next-page arguments. Never resend or infer settlement from a response.\n".to_vec());
            let between = plan(
                BETWEEN,
                endpoint,
                json!({
                "choose":{"kind":"task","taskRef":"choose-continuation","routes":{"next":"navigate","final":"done"}},
                "navigate":{"kind":"task","taskRef":"navigate-next","onVerified":"done"},
                "done":{"kind":"end","outcome":"succeeded"}}),
                "choose",
            );
            members
                .files
                .insert(format!("{BETWEEN}.flow.yaml"), encode(&between)?);
        }
        let mut profile: Value = serde_json::from_slice(&members.files["collection-profile.json"])?;
        profile["schema"] = json!(N1_OFFER_PROFILE);
        profile["betweenFlow"] = if suffix == 0 {
            Value::Null
        } else {
            json!(format!("{BETWEEN}.flow.yaml"))
        };
        profile["navigationPurpose"] = json!("navigate-next");
        profile["fixturePurpose"] = json!("catalog-link");
        profile["navigationSupported"] = json!(false);
        profile["runtimeAdmissionSupported"] = json!(false);
        profile["limits"] = json!([
            "explicit unadmitted N-1 member offer",
            "model route is not an independent last-item verifier",
            "native inBetween cursor and independent final-item refusal required",
            "matched private RPA armed observer/outcome/receipt required"
        ]);
        if suffix != 0 {
            profile["actions"].as_array_mut().context("member actions unavailable")?.push(
                json!({"flow":BETWEEN,"node":"navigate","taskRef":"navigate-next","purpose":"navigate-next","fixturePurpose":"catalog-link"}));
        }
        members
            .files
            .insert("collection-profile.json".into(), encode(&profile)?);
        members.files.remove(crate::bundle::MANIFEST_NAME);
        let manifest = json!({"format":crate::bundle::BUNDLE_FORMAT,
            "entryFlow":format!("{ROOT}.flow.yaml"),"taskRegistry":format!("{ROOT}.tasks.yaml"),
            "files":members.files.iter().map(|(path,raw)| json!({"path":path,"sha256":raw_sha256(raw)})).collect::<Vec<_>>()});
        members
            .files
            .insert(crate::bundle::MANIFEST_NAME.into(), encode(&manifest)?);
        Ok(members)
    }

    pub fn inspect_n1_offer(
        directory: &Path,
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<InspectedCollectionMembers> {
        Self::inspect_expected(
            directory,
            selection,
            Self::compile_n1_offer(selection, context, endpoint)?,
        )
    }

    /// Separate matched format. Historical v1/v2 member bytes are unchanged.
    /// These static requirements are not current authority or runtime evidence.
    pub fn compile_matched(
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<Self> {
        let mut members = Self::compile_n1_offer(selection, context, endpoint)?;
        let mut profile: Value = serde_json::from_slice(&members.files["collection-profile.json"])?;
        profile["schema"] = json!(MATCHED_PROFILE);
        let object = profile
            .as_object_mut()
            .context("matched member profile unavailable")?;
        object.remove("runtimeAdmissionSupported");
        object.remove("navigationSupported");
        profile["requiredInstalledBoundary"] = json!({
            "readySchema":MATCHED_READY,"nativeProfileId":MATCHED_NATIVE_PROFILE,
            "guardSchema":MATCHED_GUARD,"mode":"private-installed-pending",
            "outcome":"native-retained-then-primitive-postcheck-before-ack",
            "workerStop":"same-process-inactive-readback"});
        profile["limits"] = json!([
            "unqualified member format; actual installed admission required",
            "full retained selection and exact new suffix only",
            "original navigation arm cutoff and no last-item navigation",
            "unknown outcome fences progress; no effect resend",
            "nonempty last-completed-page entry unsupported"
        ]);
        members
            .files
            .insert("collection-profile.json".into(), encode(&profile)?);
        members.files.remove(crate::bundle::MANIFEST_NAME);
        let manifest = json!({"format":crate::bundle::BUNDLE_FORMAT,
            "entryFlow":format!("{ROOT}.flow.yaml"),"taskRegistry":format!("{ROOT}.tasks.yaml"),
            "files":members.files.iter().map(|(path,raw)| json!({"path":path,"sha256":raw_sha256(raw)})).collect::<Vec<_>>()});
        members
            .files
            .insert(crate::bundle::MANIFEST_NAME.into(), encode(&manifest)?);
        Ok(members)
    }

    /// Explicit fresh workload profile, never a rewrite of historical v3 members.
    pub fn compile_matched_600(
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<Self> {
        let mut members = Self::compile_matched(selection, context, endpoint)?;
        let root_file = format!("{ROOT}.flow.yaml");
        let between_file = format!("{BETWEEN}.flow.yaml");
        members.files.remove("prompts/choose-continuation.md");
        for (name, bytes) in &mut members.files {
            if name.ends_with(".flow.yaml") {
                let mut plan: Value = serde_json::from_slice(bytes)?;
                plan["limits"]["timeoutS"] = json!(600);
                if *name == root_file {
                    if let Some(between) = plan
                        .pointer_mut("/nodes/items/between")
                        .and_then(Value::as_object_mut)
                    {
                        between.insert("when".into(), json!("moreItems"));
                    }
                } else if *name == between_file {
                    plan["entryStep"] = json!("navigate");
                    plan["nodes"]
                        .as_object_mut()
                        .context("workload between nodes missing")?
                        .remove("choose");
                }
                *bytes = encode(&plan)?;
            } else if name.ends_with(".tasks.yaml") {
                let mut registry: Value = serde_json::from_slice(bytes)?;
                let tasks = registry["tasks"]
                    .as_object_mut()
                    .context("workload tasks missing")?;
                tasks.remove("choose-continuation");
                for task in tasks.values_mut() {
                    task["generationTimeoutS"] = json!(45);
                }
                *bytes = encode(&registry)?;
            }
        }
        let mut profile: Value = serde_json::from_slice(&members.files["collection-profile.json"])?;
        profile["schema"] = json!(WORKLOAD_600_PROFILE);
        profile["requiredInstalledBoundary"]["nativeProfileId"] =
            json!(WORKLOAD_600_NATIVE_PROFILE);
        profile["workloadPolicy"] = json!({"profileId":"trollm.native-workload-600s-45s.v1-proposal",
            "rootAndChildTimeoutS":600,"generationTimeoutS":45,"actTimeoutS":30,
            "meaning":"prospective-engineering-selection-not-a-completion-guarantee",
            "clockAcquisitionBudget":{"startupAttempts":WORKLOAD_CLOCK_STARTUP_ATTEMPTS,
                "maxPreparationContexts":WORKLOAD_CLOCK_PREPARATION_CONTEXTS,
                "attemptsPerContext":WORKLOAD_CLOCK_ATTEMPTS_PER_CONTEXT,
                "maintenanceAttempts":WORKLOAD_CLOCK_MAINTENANCE_ATTEMPTS,
                "totalAttempts":WORKLOAD_CLOCK_TOTAL_ATTEMPTS,
                "overflow":"permanent-refusal-before-probe"}});
        members
            .files
            .insert("collection-profile.json".into(), encode(&profile)?);
        members.files.remove(crate::bundle::MANIFEST_NAME);
        let manifest = json!({"format":crate::bundle::BUNDLE_FORMAT,
            "entryFlow":format!("{ROOT}.flow.yaml"),"taskRegistry":format!("{ROOT}.tasks.yaml"),
            "files":members.files.iter().map(|(path,raw)| json!({"path":path,"sha256":raw_sha256(raw)})).collect::<Vec<_>>()});
        members
            .files
            .insert(crate::bundle::MANIFEST_NAME.into(), encode(&manifest)?);
        Ok(members)
    }

    pub fn inspect_matched_600(
        directory: &Path,
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<InspectedCollectionMembers> {
        Self::inspect_expected(
            directory,
            selection,
            Self::compile_matched_600(selection, context, endpoint)?,
        )
    }

    pub fn inspect_matched(
        directory: &Path,
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<InspectedCollectionMembers> {
        Self::inspect_expected(
            directory,
            selection,
            Self::compile_matched(selection, context, endpoint)?,
        )
    }

    /// Pure source/member inspection. Still requires actual current owner,
    /// authority, runtime and worker association before native run admission.
    pub fn inspect(
        directory: &Path,
        selection: &SelectionRecord,
        context: &[u8],
        endpoint: &str,
    ) -> Result<InspectedCollectionMembers> {
        Self::inspect_expected(
            directory,
            selection,
            Self::compile(selection, context, endpoint)?,
        )
    }

    fn inspect_expected(
        directory: &Path,
        selection: &SelectionRecord,
        expected: Self,
    ) -> Result<InspectedCollectionMembers> {
        for (name, bytes) in &expected.files {
            ensure!(
                crate::setup::read_input_evidence(directory, name, 1024 * 1024)? == *bytes,
                "collection member raw bytes differ from target recipe: {name}"
            );
        }
        let manifest = serde_json::from_slice(&expected.files[crate::bundle::MANIFEST_NAME])?;
        let (plan, registry, setup) =
            crate::bundle::inspect_materialized_bundle(directory, &manifest)?;
        trollm_core::validate::validate_authoring_provenance(&plan, &registry)?;
        trollm_core::validate::validate_plan(&plan)?;
        trollm_core::validate::validate_with_registry(&plan, &registry)?;
        setup.context("collection setup missing")?.validate(&plan)?;
        let worklist_text = std::str::from_utf8(&expected.files["prepared-worklist.json"])?;
        let mut bindings = Vec::new();
        let mut snapshot = Vec::new();
        if !selection.selection.items[selection.selection.continuation.prefix_length as usize..]
            .is_empty()
        {
            let prepared = trollm_mcp::local::prepare_worklist(&json!({"input":worklist_text}))?;
            snapshot = prepared["items"]
                .as_array()
                .context("native worklist items missing")?
                .clone();
            let pin = plan
                .subflows
                .get(BODY)
                .context("collection body missing")?
                .pin();
            for (name, purpose) in [
                ("candidate", "candidate-select"),
                ("quantity", "quantity-editor"),
                ("add", "cart-add"),
            ] {
                bindings.push(crate::collection_provider::ActionBinding {
                    plan_pin: pin.clone(),
                    node: name.into(),
                    task_ref: name.into(),
                    purpose: purpose.into(),
                    cursor_node: Some("items".into()),
                    result_artifact: "act".into(),
                });
            }
        }
        if let Some(between) = plan.subflows.get(BETWEEN) {
            bindings.push(crate::collection_provider::ActionBinding {
                plan_pin: between.pin(),
                node: "navigate".into(),
                task_ref: "navigate-next".into(),
                purpose: "navigate-next".into(),
                cursor_node: Some("items".into()),
                result_artifact: "act".into(),
            });
        }
        Ok(InspectedCollectionMembers {
            member_profile: serde_json::from_slice::<Value>(
                &expected.files["collection-profile.json"],
            )?["schema"]
                .as_str()
                .context("member profile identity unavailable")?
                .to_owned(),
            root_plan_pin: plan.pin(),
            bindings,
            expected_snapshot: snapshot,
            selection_input_json: serde_json::to_string(
                &json!({"selectionSha256":raw_sha256(&selection.original_bytes)}),
            )?,
        })
    }

    /// A fresh materialization only. No overwrite/install/network/runtime action.
    pub fn write_new(&self, directory: &Path) -> Result<()> {
        ensure!(
            directory.is_absolute(),
            "member output directory must be absolute"
        );
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        for (name, raw) in &self.files {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().context("member parent unavailable")?)?;
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path)?.write_all(raw)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod controls {
    use super::*;
    #[test]
    fn explicit_600_workload_inspects_suffixes_and_refuses_routing_or_cap_drift() {
        for (variant, prefix) in [
            ("original", 0),
            ("subset", 0),
            ("empty", 0),
            ("changed", 0),
            ("original", 1),
            ("original", 2),
            ("original", 3),
        ] {
            let selection = selected(variant, prefix);
            let endpoint = "https://api.e2llm.dev/mcp";
            let legacy = CollectionMembers::compile_matched(
                &selection,
                b"controlled source context",
                endpoint,
            )
            .unwrap();
            let members = CollectionMembers::compile_matched_600(
                &selection,
                b"controlled source context",
                endpoint,
            )
            .unwrap();
            let directory = std::env::temp_dir().join(format!(
                "workload-600-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            members.write_new(&directory).unwrap();
            let inspected = CollectionMembers::inspect_matched_600(
                &directory,
                &selection,
                b"controlled source context",
                endpoint,
            )
            .unwrap();
            assert_eq!(inspected.member_profile, WORKLOAD_600_PROFILE);
            assert!(CollectionMembers::inspect_matched(
                &directory,
                &selection,
                b"controlled source context",
                endpoint
            )
            .is_err());
            for (name, bytes) in &members.files {
                if name.ends_with(".flow.yaml") {
                    let plan: Value = serde_json::from_slice(bytes).unwrap();
                    let old: Value = serde_json::from_slice(&legacy.files[name]).unwrap();
                    assert_eq!(plan["limits"]["timeoutS"], 600);
                    assert_eq!(old["limits"]["timeoutS"], 300);
                    assert_eq!(plan["mcp"], old["mcp"]);
                } else if name.ends_with(".tasks.yaml") {
                    let registry: Value = serde_json::from_slice(bytes).unwrap();
                    for task in registry["tasks"].as_object().unwrap().values() {
                        assert_eq!(task["generationTimeoutS"], 45);
                    }
                }
            }
            let name = format!("{ROOT}.flow.yaml");
            let root: Value = serde_json::from_slice(&members.files[&name]).unwrap();
            let between_name = format!("{BETWEEN}.flow.yaml");
            let registry: Value =
                serde_json::from_slice(&members.files[&format!("{ROOT}.tasks.yaml")]).unwrap();
            assert!(registry["tasks"].get("choose-continuation").is_none());
            assert!(!members.files.contains_key("prompts/choose-continuation.md"));
            let suffix_count = selection.selection.items.len() - prefix;
            assert_eq!(inspected.expected_snapshot.len(), suffix_count);
            if suffix_count == 0 {
                assert!(root["nodes"].get("items").is_none());
                assert!(!members.files.contains_key(&between_name));
                assert!(inspected.bindings.is_empty());
            } else {
                assert_eq!(
                    root["nodes"]["items"]["between"],
                    json!({"flowRef":BETWEEN,"when":"moreItems"})
                );
                let between: Value = serde_json::from_slice(&members.files[&between_name]).unwrap();
                let old: Value = serde_json::from_slice(&legacy.files[&between_name]).unwrap();
                assert_eq!(between["entryStep"], "navigate");
                assert!(between["nodes"].get("choose").is_none());
                assert_eq!(between["nodes"]["navigate"], old["nodes"]["navigate"]);
                assert_eq!(old["entryStep"], "choose");
                let navigation = &registry["tasks"]["navigate-next"];
                assert_eq!(
                    navigation["toolArguments"]["e2llm.act"],
                    json!({"fixed":{"action":"navigate"},"modelFields":[]})
                );
                assert_eq!(navigation["mutations"]["onUncertain"], "fail-run");
                assert!(inspected.bindings.iter().any(|binding| {
                    binding.node == "navigate"
                        && binding.task_ref == "navigate-next"
                        && binding.purpose == "navigate-next"
                        && binding.cursor_node.as_deref() == Some("items")
                        && binding.plan_pin.starts_with(BETWEEN)
                }));

                // Substituting the old between flow must fail exact reconstruction.
                std::fs::write(directory.join(&between_name), &legacy.files[&between_name])
                    .unwrap();
                assert!(CollectionMembers::inspect_matched_600(
                    &directory,
                    &selection,
                    b"controlled source context",
                    endpoint
                )
                .is_err());
                std::fs::write(directory.join(&between_name), &members.files[&between_name])
                    .unwrap();
            }

            // A producer cannot silently change the cap or remove the predicate.
            let mut changed = root.clone();
            changed["limits"]["timeoutS"] = json!(601);
            for changed in [
                Some(changed),
                (suffix_count > 0).then(|| {
                    let mut altered = root.clone();
                    altered["nodes"]["items"]["between"] = json!({"flowRef":BETWEEN});
                    altered
                }),
            ]
            .into_iter()
            .flatten()
            {
                std::fs::write(directory.join(&name), encode(&changed).unwrap()).unwrap();
                assert!(CollectionMembers::inspect_matched_600(
                    &directory,
                    &selection,
                    b"controlled source context",
                    endpoint
                )
                .is_err());
            }
            std::fs::remove_dir_all(directory).unwrap();
            assert_eq!(
                legacy.files,
                CollectionMembers::compile_matched(
                    &selection,
                    b"controlled source context",
                    endpoint
                )
                .unwrap()
                .files
            );
        }
    }
    #[test]
    #[cfg(feature = "stub")]
    fn source_entry_inspects_before_refusing_uninstalled_worker_for_all_suffix_shapes() {
        for (variant, prefix, matched) in [
            ("original", 0, false),
            ("empty", 0, false),
            ("original", 3, false),
            ("original", 0, true),
            ("empty", 0, true),
            ("original", 3, true),
        ] {
            let selection = selected(variant, prefix);
            let compile = if matched {
                CollectionMembers::compile_matched
            } else {
                CollectionMembers::compile
            };
            let members = compile(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let installation = raw_sha256(&members.files["manifest.json"]);
            let flow = raw_sha256(&members.files["selected-collection.flow.yaml"]);
            let base = std::env::temp_dir().join(format!(
                "collection-source-entry-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            let revisions = base.join("bundle-revisions");
            std::fs::create_dir_all(&revisions).unwrap();
            members.write_new(&revisions.join(&installation)).unwrap();
            let source = CollectionSource {
                schema_version: SOURCE_SCHEMA.into(),
                origin: "DECLARED".into(),
                owner: selection.selection.owner.clone(),
                fixture_id: "controlled-fixture".into(),
                version_id: "controlled-version".into(),
                selection_ref: selection.selection_ref.clone(),
                selection_raw_sha256: raw_sha256(&selection.original_bytes),
                profile_id: if matched { MATCHED_PROFILE } else { PROFILE }.into(),
                profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
                installation_sha256: installation.clone(),
                qualification: "unqualified".into(),
            };
            let input = serde_json::to_string(
                &json!({"selectionSha256":raw_sha256(&selection.original_bytes)}),
            )
            .unwrap();
            let setup: crate::setup::FlowSetup =
                serde_json::from_slice(&members.files["setup.json"]).unwrap();
            let admitted =
                crate::setup::admit_source_input(setup.input.as_ref().unwrap(), &input, &input)
                    .unwrap();
            let association = json!({
                "inputAdmission":admitted,
                "installation":{"installationSha256":installation,"flowSha256":flow,"archiveSha256":"synthetic-container"},
                "descriptor":{"source":source,"identity":{"manifestSha256":installation,"entryFlowSha256":flow,"sha256":"synthetic-container"}}
            });
            for mode in ["valid", "profile-drift", "input-drift", "admission-missing"] {
                let mut association = association.clone();
                let mut supplied_input = input.clone();
                if mode == "profile-drift" {
                    association["descriptor"]["source"]["profileSha256"] = json!("0".repeat(64));
                }
                if mode == "input-drift" {
                    supplied_input =
                        serde_json::to_string(&json!({"selectionSha256":"0".repeat(64)})).unwrap();
                    association["inputAdmission"] = crate::setup::admit_source_input(
                        setup.input.as_ref().unwrap(),
                        &supplied_input,
                        &supplied_input,
                    )
                    .unwrap();
                }
                if mode == "admission-missing" {
                    association
                        .as_object_mut()
                        .unwrap()
                        .remove("inputAdmission");
                }
                let error = match crate::run::spawn_source_run(
                    base.clone(),
                    flow.clone(),
                    supplied_input,
                    None,
                    None,
                    None,
                    Some((installation.clone(), association)),
                ) {
                    Ok(_) => panic!("uninstalled collection worker must not start"),
                    Err(error) => format!("{error:#}"),
                };
                assert!(
                    error.contains(match mode {
                        "profile-drift" => "collection source profile drift",
                        "input-drift" =>
                            "collection admitted input differs from immutable selected roster",
                        "admission-missing" => "declared source requires native input admission",
                        _ => "collection installed authority/preparer/worker binding unavailable",
                    }),
                    "{variant}/{prefix}/{mode}: {error}"
                );
                assert!(!base.join("runs").exists(), "refusal created a run/worker");
            }
            std::fs::remove_dir_all(base).unwrap();
        }
    }
    fn selected(variant: &str, prefix: usize) -> SelectionRecord {
        let mut body = crate::collection::tests::body();
        body["contextRef"]["sha256"] = json!(raw_sha256(b"controlled source context"));
        body["variant"] = json!(variant);
        if variant == "changed" {
            body["items"][0]["originalItemId"] = json!("changed-ID");
            body["items"][0]["quantity"] = json!(3);
            body["items"][0]["quantityText"] = json!("3");
        }
        if variant == "empty" {
            body["items"] = json!([]);
        }
        if variant == "subset" {
            body["items"] = json!([body["items"][1].clone()]);
            body["items"][0]["admittedOrder"] = json!(0);
        }
        if prefix > 0 {
            body["continuation"]["prefixLength"] = json!(prefix);
            body["continuation"]["knownCommittedItemIds"] =
                json!(body["items"].as_array().unwrap()[..prefix]
                    .iter()
                    .map(|v| v["originalItemId"].clone())
                    .collect::<Vec<_>>());
            body["continuation"]["parentOrigin"] = json!("offline-declared");
            body["continuation"]["parentCheckpointRef"] = body["applicationRef"].clone();
        }
        crate::collection::tests::decode(&body).unwrap()
    }
    #[test]
    fn actual_native_loaders_validate_each_variant_and_suffix_members() {
        for (variant, prefix) in [
            ("original", 0),
            ("changed", 0),
            ("subset", 0),
            ("empty", 0),
            ("original", 1),
            ("original", 2),
            ("original", 3),
        ] {
            let selection = selected(variant, prefix);
            let members = CollectionMembers::compile(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let dir = std::env::temp_dir().join(format!(
                "collection-offer-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            members.write_new(&dir).unwrap();
            let manifest =
                serde_json::from_slice(&members.files[crate::bundle::MANIFEST_NAME]).unwrap();
            let (plan, registry, setup) =
                crate::bundle::inspect_materialized_bundle(&dir, &manifest).unwrap();
            trollm_core::validate::validate_authoring_provenance(&plan, &registry).unwrap();
            trollm_core::validate::validate_plan(&plan).unwrap();
            trollm_core::validate::validate_with_registry(&plan, &registry).unwrap();
            setup.unwrap().validate(&plan).unwrap();
            let worklist: Value =
                serde_json::from_slice(&members.files["prepared-worklist.json"]).unwrap();
            assert_eq!(
                worklist["items"],
                serde_json::to_value(&selection.selection.items[prefix..]).unwrap()
            );
            assert_eq!(members.files["selection.json"], selection.original_bytes);
            let inspected = CollectionMembers::inspect(
                &dir,
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let mut source = CollectionSource {
                schema_version: SOURCE_SCHEMA.into(),
                origin: "DECLARED".into(),
                owner: selection.selection.owner.clone(),
                fixture_id: "controlled-fixture".into(),
                version_id: "controlled-version".into(),
                selection_ref: selection.selection_ref.clone(),
                selection_raw_sha256: raw_sha256(&selection.original_bytes),
                profile_id: PROFILE.into(),
                profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
                installation_sha256: raw_sha256(&members.files["manifest.json"]),
                qualification: "unqualified".into(),
            };
            assert_eq!(
                source
                    .inspect(&dir, "https://api.e2llm.dev/mcp")
                    .unwrap()
                    .root_plan_pin,
                plan.pin()
            );
            source.profile_sha256 = "0".repeat(64);
            assert!(source.inspect(&dir, "https://api.e2llm.dev/mcp").is_err());
            assert_eq!(inspected.root_plan_pin, plan.pin());
            assert_eq!(
                inspected.expected_snapshot.len(),
                selection.selection.items.len() - prefix
            );
            assert!(!plan.pin().contains("0000000000000000"));
            if !selection.selection.items[prefix..].is_empty() {
                let result = trollm_mcp::local::prepare_worklist(&json!({"input":std::str::from_utf8(&members.files["prepared-worklist.json"]).unwrap()})).unwrap();
                assert_eq!(result["items"][0]["value"], worklist["items"][0]);
                assert!(plan.subflows[BODY]
                    .pin()
                    .contains(&raw_sha256(&members.files[&format!("{BODY}.flow.yaml")])));
                for task in ["candidate", "quantity", "add"] {
                    assert!(registry.tasks[task].mutations.is_some());
                }
            } else {
                assert!(registry.tasks.is_empty());
                assert_eq!(plan.entry_step, "done");
            }
            assert!(
                members.write_new(&dir).is_err(),
                "never overwrite an existing materialization"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn independently_serialized_invalid_variant_positions_refuse() {
        for mode in 0..4 {
            let mut body = crate::collection::tests::body();
            body["contextRef"]["sha256"] = json!(raw_sha256(b"controlled source context"));
            match mode {
                0 => {
                    body["items"] = json!([body["items"][1].clone()]);
                    body["items"][0]["admittedOrder"] = json!(0);
                }
                1 => body["variant"] = json!("subset"),
                2 => {
                    body["variant"] = json!("subset");
                    body["items"] = json!([body["items"][2].clone(), body["items"][0].clone()]);
                    body["items"][0]["admittedOrder"] = json!(0);
                    body["items"][1]["admittedOrder"] = json!(1);
                }
                _ => {
                    body["variant"] = json!("changed");
                    body["items"].as_array_mut().unwrap().swap(0, 1);
                    body["items"][0]["admittedOrder"] = json!(0);
                    body["items"][1]["admittedOrder"] = json!(1);
                }
            }
            let selection = crate::collection::tests::decode(&body).unwrap();
            assert!(CollectionMembers::compile(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp"
            )
            .is_err());
        }
    }
    #[test]
    fn n1_offer_uses_existing_loaders_and_keeps_runtime_admission_refused() {
        for (variant, prefix) in [
            ("original", 0),
            ("changed", 0),
            ("subset", 0),
            ("empty", 0),
            ("original", 1),
            ("original", 2),
            ("original", 3),
        ] {
            let selection = selected(variant, prefix);
            let original = CollectionMembers::compile(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let members = CollectionMembers::compile_n1_offer(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let suffix = selection.selection.items.len() - prefix;
            assert_eq!(
                members.files["selection.json"],
                original.files["selection.json"]
            );
            assert_eq!(
                members.files["prepared-worklist.json"],
                original.files["prepared-worklist.json"]
            );
            let profile: Value =
                serde_json::from_slice(&members.files["collection-profile.json"]).unwrap();
            assert_eq!(profile["schema"], N1_OFFER_PROFILE);
            assert_eq!(profile["navigationPurpose"], "navigate-next");
            assert_eq!(profile["fixturePurpose"], "catalog-link");
            assert_eq!(profile["navigationSupported"], false);
            assert_eq!(profile["runtimeAdmissionSupported"], false);
            let directory = std::env::temp_dir().join(format!(
                "collection-n1-offer-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            members.write_new(&directory).unwrap();
            let inspected = CollectionMembers::inspect_n1_offer(
                &directory,
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            assert_eq!(inspected.expected_snapshot.len(), suffix);
            assert_eq!(inspected.bindings.len(), if suffix == 0 { 0 } else { 4 });
            assert!(
                CollectionMembers::inspect(
                    &directory,
                    &selection,
                    b"controlled source context",
                    "https://api.e2llm.dev/mcp"
                )
                .is_err(),
                "N-1 offer must not silently become the frozen v1 recipe"
            );
            let source = CollectionSource {
                schema_version: SOURCE_SCHEMA.into(),
                origin: "DECLARED".into(),
                owner: selection.selection.owner.clone(),
                fixture_id: "declared-offer".into(),
                version_id: "declared-offer".into(),
                selection_ref: selection.selection_ref.clone(),
                selection_raw_sha256: raw_sha256(&selection.original_bytes),
                profile_id: N1_OFFER_PROFILE.into(),
                profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
                installation_sha256: raw_sha256(&members.files["manifest.json"]),
                qualification: "unqualified".into(),
            };
            assert!(
                source
                    .inspect(&directory, "https://api.e2llm.dev/mcp")
                    .is_err(),
                "public source/runtime admission of unmatched N-1 must remain unavailable"
            );
            if suffix != 0 {
                let between: Value =
                    serde_json::from_slice(&members.files[&format!("{BETWEEN}.flow.yaml")])
                        .unwrap();
                assert_eq!(
                    between["nodes"]["choose"]["routes"],
                    json!({"next":"navigate","final":"done"})
                );
                let registry: Value =
                    serde_json::from_slice(&members.files[&format!("{ROOT}.tasks.yaml")]).unwrap();
                assert_eq!(
                    registry["tasks"]["navigate-next"]["toolArguments"]["e2llm.act"],
                    json!({"fixed":{"action":"navigate"},"modelFields":[]})
                );
                assert!(inspected
                    .bindings
                    .iter()
                    .any(|binding| binding.purpose == "navigate-next"
                        && binding.cursor_node.as_deref() == Some("items")
                        && binding.plan_pin.starts_with(BETWEEN)));
            }
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn matched_source_inspects_all_variants_and_exact_suffix_without_admitting_legacy_offer() {
        for (variant, prefix) in [
            ("original", 0),
            ("changed", 0),
            ("subset", 0),
            ("empty", 0),
            ("original", 1),
            ("original", 2),
            ("original", 3),
        ] {
            let selection = selected(variant, prefix);
            let members = CollectionMembers::compile_matched(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let legacy = CollectionMembers::compile_n1_offer(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            assert_eq!(
                members.files["selection.json"],
                legacy.files["selection.json"]
            );
            assert_eq!(
                members.files["prepared-worklist.json"],
                legacy.files["prepared-worklist.json"]
            );
            assert_ne!(
                members.files["manifest.json"],
                legacy.files["manifest.json"]
            );
            let directory = std::env::temp_dir().join(format!(
                "collection-matched-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            members.write_new(&directory).unwrap();
            let source = CollectionSource {
                schema_version: SOURCE_SCHEMA.into(),
                origin: "DECLARED".into(),
                owner: selection.selection.owner.clone(),
                fixture_id: "matched-declared".into(),
                version_id: "matched-declared".into(),
                selection_ref: selection.selection_ref.clone(),
                selection_raw_sha256: raw_sha256(&selection.original_bytes),
                profile_id: MATCHED_PROFILE.into(),
                profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
                installation_sha256: raw_sha256(&members.files["manifest.json"]),
                qualification: "unqualified".into(),
            };
            let inspected = source
                .inspect(&directory, "https://api.e2llm.dev/mcp")
                .unwrap();
            assert_eq!(inspected.member_profile, MATCHED_PROFILE);
            assert_eq!(
                inspected.expected_snapshot.len(),
                selection.selection.items.len() - prefix
            );
            assert_eq!(
                inspected.bindings.len(),
                if inspected.expected_snapshot.is_empty() {
                    0
                } else {
                    4
                }
            );
            assert!(CollectionMembers::inspect_n1_offer(
                &directory,
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp"
            )
            .is_err());
            let profile: Value =
                serde_json::from_slice(&members.files["collection-profile.json"]).unwrap();
            assert_eq!(
                profile["requiredInstalledBoundary"]["guardSchema"],
                MATCHED_GUARD
            );
            assert!(profile.get("navigationSupported").is_none());
            std::fs::write(
                directory.join("collection-profile.json"),
                &legacy.files["collection-profile.json"],
            )
            .unwrap();
            assert!(source
                .inspect(&directory, "https://api.e2llm.dev/mcp")
                .is_err());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn context_endpoint_and_original_selection_drift_refuse_before_members() {
        let mut selection = selected("original", 0);
        assert!(
            CollectionMembers::compile(&selection, b"different", "https://api.e2llm.dev/mcp")
                .is_err()
        );
        assert!(CollectionMembers::compile(
            &selection,
            b"controlled source context",
            "https://other.invalid/mcp"
        )
        .is_err());
        selection.selection.items[0].quantity_text = "3".into();
        assert!(CollectionMembers::compile(
            &selection,
            b"controlled source context",
            "https://api.e2llm.dev/mcp"
        )
        .is_err());
    }
}
