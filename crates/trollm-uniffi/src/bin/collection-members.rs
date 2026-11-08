//! Compile target member bytes only; no installation, credentials or effects.
use anyhow::{ensure, Context, Result};
use std::path::Path;
use trollm_uniffi::collection::{OwnedRef, Owner, SelectionRecord};
use trollm_uniffi::collection_members::CollectionMembers;
fn read(path: &str, maximum: usize) -> Result<Vec<u8>> {
    let path = Path::new(path);
    ensure!(
        path.is_absolute() && std::fs::symlink_metadata(path)?.is_file(),
        "input must be an absolute regular file"
    );
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= maximum,
        "member compiler input exceeds bound"
    );
    Ok(bytes)
}
fn run() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let n1_offer = args.first().is_some_and(|value| value == "--n1-offer");
    let matched = args
        .first()
        .is_some_and(|value| value == "--matched-pending");
    if n1_offer || matched {
        args.remove(0);
    }
    ensure!(args.len() == 6, "expected six member compiler arguments");
    let reference: OwnedRef = serde_json::from_slice(&read(&args[1], 4096)?)?;
    let owner: Owner = serde_json::from_slice(&read(&args[2], 4096)?)?;
    let selection = SelectionRecord::decode(&read(&args[0], 65536)?, &reference, &owner)?;
    let context = read(&args[3], 2048)?;
    let members = if matched {
        CollectionMembers::compile_matched(&selection, &context, &args[4])?
    } else if n1_offer {
        CollectionMembers::compile_n1_offer(&selection, &context, &args[4])?
    } else {
        CollectionMembers::compile(&selection, &context, &args[4])?
    };
    members.write_new(Path::new(&args[5]))?;
    let inspected = if matched {
        CollectionMembers::inspect_matched(Path::new(&args[5]), &selection, &context, &args[4])?
    } else if n1_offer {
        CollectionMembers::inspect_n1_offer(Path::new(&args[5]), &selection, &context, &args[4])?
    } else {
        CollectionMembers::inspect(Path::new(&args[5]), &selection, &context, &args[4])?
    };
    println!(
        "{}",
        serde_json::json!({"profile":inspected.member_profile,"manifestSha256":trollm_uniffi::collection::raw_sha256(&members.files["manifest.json"]),"files":members.files.len(),"rootPlanPin":inspected.root_plan_pin,"bindings":inspected.bindings.iter().map(|b|serde_json::json!({"planPin":b.plan_pin,"node":b.node,"taskRef":b.task_ref,"purpose":b.purpose,"cursorNode":b.cursor_node,"resultArtifact":b.result_artifact})).collect::<Vec<_>>(),"runtimeAdmissionSupported":false})
    );
    Ok(())
}
fn main() {
    if let Err(error) = run().context("collection member compilation refused") {
        eprintln!("{error:#}\nUsage: collection-members [--n1-offer|--matched-pending] /abs/selection.json /abs/selection-ref.json /abs/owner.json /abs/context.txt https://api.e2llm.dev/mcp /abs/new-output");
        std::process::exit(2);
    }
}
