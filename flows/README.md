# Flow reference material

These files illustrate the execution-plan and task-registry contracts. They are
source inputs, rather than prebuilt bundles ready to install in the Android app.

For the planned direct-authoring experience, see the
[LLM authoring guide](../docs/FLOW-AUTHORING.md) and
[flow contract direction](../docs/FLOW-CONTRACT-DIRECTION.md). They describe
future metadata and tooling; the examples here continue to use the current v1
contract until its schema and consumers are updated together.

| Material | Status | Files |
| --- | --- | --- |
| Minimal summary | Directly authored, tool-free reference exercised with a scripted model | [plan](examples/minimal-summary.flow.yaml), [task registry](tasks/minimal-summary.tasks.yaml), [prompt](tasks/prompts/minimal-summary.md) |
| Ticket triage | Complete reference used by the core conformance test with scripted models and a test gateway | [ticket-triage](examples/ticket-triage.flow.yaml), [process-item](examples/process-item.flow.yaml), [return-to-list](examples/return-to-list.flow.yaml), [task registry](tasks/tickets.tasks.yaml), and [prompt files](tasks/prompts/) |
| Procurement | Design sketch; schema and transitions can be checked, but the procurement task registry and `evaluate-product` subflow are missing | [procurement](examples/procurement.flow.yaml) |

The ticket reference requires a compatible tracker MCP server for a real run.
Its example server address and baseline pins need deployment-specific values.
The conformance test establishes behavior with test doubles; it does not establish
that this reference is ready for a particular live tracker.

[validate-examples.py](../scripts/validate-examples.py) checks the plan schemas,
transitions, complete-reference subflow/task references and prompt paths. The
[conformance test](../crates/trollm-core/tests/conformance.rs) exercises the
ticket reference with scripted dependencies.

## Minimal summary

This is the smallest complete authoring example: user text → one coder task →
prose `report` → succeeded end. The registry grants no tools and the prompt
uses the existing `{{ user_input }}` binding. It demonstrates output plumbing,
not independent verification of the summary's accuracy.

No `sourceBaseline` is supplied: a browser recording is not required. The
updated v1 parser accepts this omission; older builds, including the previously
smoke-tested `05850a9` APK, still require the field. Use a build containing this
change before packaging the example for Android.

From the repository root:

```sh
python3 scripts/validate-examples.py
cargo test --locked -p trollm-core --test direct_authoring --no-default-features
```

The test loads these actual files and runs the real executor with a scripted
model, checking the input binding, final report and absence of tool calls. A
real run still requires an app-configured coder model. This is source material,
not a bundled model or a claim of real inference acceptance.

## Android packaging

Android installation requires an uncompressed ustar archive with a manifest
listing the entry flow, registry and per-file SHA-256 hashes. The format is
implemented in [bundle.rs](../crates/trollm-uniffi/src/bundle.rs) and
[tar.rs](../crates/trollm-uniffi/src/tar.rs); no standalone packing command is
provided yet. See the [Android guide](../apps/android/README.md) for installation
and the [architecture](../docs/ARCHITECTURE-ANDROID.md) for authoring contracts.
