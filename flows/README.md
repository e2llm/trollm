# Flow reference material

These files illustrate the execution-plan and task-registry contracts. The
`examples/` and `tasks/` trees contain source references; `starters/` contains
the sources of the installable bundles shipped with the Android app.

New to the format? Start with [Anatomy of a flow](../docs/ANATOMY-OF-A-FLOW.md),
which connects the minimal plan, registry, prompt and scripted executor test.
For larger procedures, use the [repeated step groups](../docs/FLOW-PRACTICES.md#repeated-step-groups)
to prepare intent once, select from original evidence, and act/observe/assess.
They are authoring patterns using existing tasks and routes, not new node types.

When writing or revising prompts, follow [Flow prompting](../docs/FLOW-PROMPTING.md):
ask the intended coder and reasoner separately to help phrase their tasks, review
the suggestions and test both successful selections and appropriate declines.
The guide includes copyable authoring questions and a selection-or-decline example.

To keep separate results from the same remote read tool, use the implemented
[task-specific result artifact](../docs/TASK-RESULT-ARTIFACTS.md) field. It names
native results without model copying; the guide explains restrictions, replacement
and the difference between a cached result and a fresh observation.

In the current v1 runtime, `input.promptTemplate` is reserved metadata and is not
rendered. Task-registry `prompt` files supply model instructions and can reference
`{{ user_input }}`. Existing example values are retained for compatibility; editing
them alone does not change the generated prompt. MCP transport `retry` accepts
only its single-attempt defaults (`attempts: 1, backoffS: 0`); other values reject
at validation. `backgroundFetch: all` performs a bounded synchronous fetch before
returning a page, not a detached background download.

The [LLM authoring guide](../docs/FLOW-AUTHORING.md) describes the current
task-design and testing method, including a
[starter prompt exercise](../docs/FLOW-AUTHORING.md#make-one-small-change-to-an-existing-starter).
Its integrated authoring pack and the additions in
[flow contract direction](../docs/FLOW-CONTRACT-DIRECTION.md) remain proposals.
The examples here use the current v1 contract until its schema and consumers are
updated together.

| Material | Status | Files |
| --- | --- | --- |
| Minimal summary | Directly authored, tool-free reference exercised with a scripted model | [plan](examples/minimal-summary.flow.yaml), [task registry](tasks/minimal-summary.tasks.yaml), [prompt](tasks/prompts/minimal-summary.md) |
| Readable record lookup | Coder selects through original text, native lookup preserves the complete record, reasoner reviews it; synthetic teaching records and scripted tests | [plan](examples/record-lookup.flow.yaml), [registry](tasks/record-lookup.tasks.yaml), [walkthrough](../docs/FLOW-PRACTICES.md#worked-example-readable-reference-to-original-record), [test](../crates/trollm-core/tests/record_lookup.rs) |
| Prose calculation | Builtin exact arithmetic, then tool-free reasoner advice and a coder report; scripted models with the real gateway | [plan](examples/prose-calculation.flow.yaml), [task registry](tasks/prose-calculation.tasks.yaml), [prompts](tasks/prompts/), [executor test](../crates/trollm-core/tests/prose_calculation.rs) |
| Check a total | Installable local-tool starter with a user-supplied budget and the same evidence handoff | [plan](starters/check-total/check-total.flow.yaml), [task registry](starters/check-total/check-total.tasks.yaml), [setup](starters/check-total/setup.json), [executor test](../crates/trollm-core/tests/local_starter.rs) |
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

## Prose calculation

Supply literal text such as `My cart has items at 39.99 and 40.10. My budget is
80.` The coder selects two prices and calls builtin `local.calculate` to add
them. A second coder task compares the exact saved sum `80.09` with the fixed
teaching budget `80`, producing `greater`. The tool-free reasoner receives the
original prose and both exact calculation payloads, then the coder turns its
advice into a final report with no granted tools.

This example needs no MCP server, credentials, browser, or model weights for its
scripted test. A real run needs both configured model slots and a runtime with
the builtin calculator. It is source material, not an Android starter bundle.
It expects two item prices and a budget of 80; changing the prose alone does not
change that fixed budget. Calculator inputs are decimal strings, bounded to
12 integer and 6 fractional digits. The result artifact `calculation.json` is
replaced by the comparison, while each task's bound outputs retain its own
payload for later prompts.

The calculation tasks use existing task allowlists, immutable argument bindings,
`completion: tool-result`, result validation and complete-artifact requirements.
Exactly one successful tool call completes each calculation task without asking
the model to restate its result. Deterministic arithmetic does not verify that
the coder selected the right facts or that model advice is accurate. The final
advice does not authorize a purchase or any external action.

```sh
python3 scripts/validate-examples.py
cargo test --locked -p trollm-core --test prose_calculation --no-default-features
```

The test uses the production executor and gateway with scripted model slots and
no remote servers. It checks exact operand/result handoff, the original prose,
and failures for missing calls, denied tools and conflicting fixed arguments;
it does not establish real-model extraction quality or device acceptance.

## Coder-directed routing

The [adaptive routing example](examples/adaptive-routing.flow.yaml) uses a
tool-free reasoner to assess user text, then a coder calls `flow.choose` to
select an authored summarization, unresolved-report or reconsideration route.
Two report nodes reuse one task with different `promptSupplement` instructions.
The runtime does not inspect the reasoner's prose or verify summary accuracy.
This example exercises the [adaptive-flow contract](../docs/ADAPTIVE-FLOWS.md)
with scripted models and no remote tools; it is not device acceptance.

```sh
cargo test --locked -p trollm-core --test adaptive_example --no-default-features
```

## Android packaging

**Check a total** packages the calculation pattern for the app. Enter text such
as `Two items cost 39.99 and 40.10.` and a separate budget such as `75`.
Unlike the teaching example's fixed budget of 80, the starter binds comparison
to your budget field. It requires exactly two item amounts and has no remote
servers or credentials. Its setup pins the same Qwen/Gemma assets as the browser
starter. See the [built-in tools contract](../docs/BUILTIN-TOOLS.md) for the
execution path and the limits of arithmetic verification.

Regenerate the committed starter after editing its sources:

```sh
python3 scripts/android/make_starter_bundles.py --starter check-total
python3 scripts/android/make_starter_bundles.py --check
cargo test --locked -p trollm-core --test local_starter --no-default-features
```

Android installation requires an uncompressed ustar archive with a manifest
listing the entry flow, registry and per-file SHA-256 hashes. The format is
implemented in [bundle.rs](../crates/trollm-uniffi/src/bundle.rs) and
[tar.rs](../crates/trollm-uniffi/src/tar.rs). The helper above packages the named
starters; a general command for packing arbitrary flows is not provided yet.
See the [Android guide](../apps/android/README.md) for installation
and the [architecture](../docs/ARCHITECTURE-ANDROID.md) for authoring contracts.
