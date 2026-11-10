//! Actual compiler CLI boundary only, without runtime admission or models.
use serde_json::{json, Value};
use std::process::Command;
use trollm_uniffi::collection::{raw_sha256, OwnedRef, Owner, SelectionRecord};
use trollm_uniffi::collection_members::{CollectionMembers, WORKLOAD_600_PROFILE};

#[test]
fn explicit_workload_cli_matches_native_inspection_for_four_original_input_shapes() {
    let root = std::env::temp_dir().join(format!(
        "workload-cli-{}",
        trollm_core::RunDir::under(std::path::Path::new("unused")).run_id()
    ));
    std::fs::create_dir(&root).unwrap();
    for variant in ["original", "subset", "empty", "changed"] {
        let directory = root.join(variant);
        std::fs::create_dir(&directory).unwrap();
        let reference = json!({"tenantId":"test-tenant","artifactId":format!("art_{}","a".repeat(32)),"sha256":"b".repeat(64)});
        let owner: Owner = serde_json::from_value(
            json!({"registrationId":"test-registration","tenantId":"test-tenant",
            "subject":"test-subject","workerId":"test-worker","runId":"test-source","epoch":1}),
        )
        .unwrap();
        let context = b"declared CLI context, not an execution";
        let mut selection = json!({"schema":"moray.declared-collection-selection.v1-proposal","owner":owner,
            "selectionId":format!("selected-{variant}"),"caseId":"test-case","invocationKey":"test-invocation","variant":variant,
            "source":{"sourceRef":reference,"profileId":"declared-test-only","sourceIdentitySha256":"c".repeat(64),"dependencyRefs":[reference]},
            "applicationRef":reference,"requestsRef":reference,"supportRef":reference,
            "contextRef":{"tenantId":"test-tenant","artifactId":format!("art_{}","c".repeat(32)),"sha256":raw_sha256(context)},
            "items":[{"originalItemId":"x9","inputPosition":0,"admittedOrder":0,"quantity":1,"quantityText":"1"},
                {"originalItemId":"k2","inputPosition":1,"admittedOrder":1,"quantity":2,"quantityText":"2"},
                {"originalItemId":"z7","inputPosition":2,"admittedOrder":2,"quantity":3,"quantityText":"3"}],
            "continuation":{"parentCheckpointRef":null,"parentTrialRef":null,"knownCommittedItemIds":[],"prefixLength":0,"parentOrigin":"none"}});
        if variant == "empty" {
            selection["items"] = json!([]);
        }
        if variant == "subset" {
            selection["items"] = json!([selection["items"][1].clone()]);
            selection["items"][0]["admittedOrder"] = json!(0);
        }
        if variant == "changed" {
            selection["items"][0]["quantity"] = json!(3);
            selection["items"][0]["quantityText"] = json!("3");
        }
        let raw = trollm_mcp::preparation::js_sorted_json_bytes(&selection).unwrap();
        let selected_ref = OwnedRef {
            tenant_id: "test-tenant".into(),
            artifact_id: format!("art_{}", "d".repeat(32)),
            sha256: raw_sha256(&raw),
        };
        for (name, bytes) in [
            ("selection.json", raw.clone()),
            ("owner.json", serde_json::to_vec(&owner).unwrap()),
            ("reference.json", serde_json::to_vec(&selected_ref).unwrap()),
            ("context.txt", context.to_vec()),
        ] {
            std::fs::write(directory.join(name), bytes).unwrap();
        }
        let output = directory.join("new-members");
        let args = [
            directory.join("selection.json"),
            directory.join("reference.json"),
            directory.join("owner.json"),
            directory.join("context.txt"),
        ];
        let invoke = || {
            let mut command = Command::new(env!("CARGO_BIN_EXE_collection-members"));
            command
                .arg("--matched-pending-600")
                .args(&args)
                .arg("https://api.e2llm.dev/mcp")
                .arg(&output);
            command.output().unwrap()
        };
        let result = invoke();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let projection: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(projection["profile"], WORKLOAD_600_PROFILE);
        assert_eq!(projection["runtimeAdmissionSupported"], false);
        let record = SelectionRecord::decode(&raw, &selected_ref, &owner).unwrap();
        let inspected = CollectionMembers::inspect_matched_600(
            &output,
            &record,
            context,
            "https://api.e2llm.dev/mcp",
        )
        .unwrap();
        assert_eq!(projection["rootPlanPin"], inspected.root_plan_pin);
        let original_manifest = std::fs::read(output.join("manifest.json")).unwrap();
        assert_eq!(projection["manifestSha256"], raw_sha256(&original_manifest));
        assert_eq!(
            projection["profileSha256"],
            raw_sha256(&std::fs::read(output.join("collection-profile.json")).unwrap())
        );
        assert!(
            !invoke().status.success(),
            "existing output must not be overwritten"
        );
        assert_eq!(
            std::fs::read(output.join("manifest.json")).unwrap(),
            original_manifest
        );
        assert!(!root.join("runs").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}
