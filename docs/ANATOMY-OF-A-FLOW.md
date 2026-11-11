# Anatomy of a flow: the current v1 runtime

For the product/role picture, [Learn trollm](LEARN-TROLLM.md) follows Check a total.
This walkthrough deliberately removes tools and advice to isolate the source
format and one execution path before introducing those concerns again.

Start with [minimal summary](../flows/examples/minimal-summary.flow.yaml): user
text goes to one tool-free coder task, its prose becomes a `report`, and the
executor reaches a succeeded end node. No browser recording, MCP connection or
reasoner is needed. This walkthrough follows the existing source files and a
scripted executor test; it is not an Android installation quickstart.

## The three files

| File | What it controls |
| --- | --- |
| [Execution plan](../flows/examples/minimal-summary.flow.yaml) | Entry node, task reference, transitions, model slots and run budgets. |
| [Task registry](../flows/tasks/minimal-summary.tasks.yaml) | The task's model slot, prompt path, tool grants, generation-loop bound and output contract. |
| [Prompt](../flows/tasks/prompts/minimal-summary.md) | Instructions sent to the model, with the supplied text inserted at `{{ user_input }}`. |

The plan and registry use separate versioned formats:
`ledger.execution-plan.v1` and `ledger.task-registry.v1`. Their `id` and `version`
identify the authored documents; they do not select model weights. The caller
loads the plan and registry together. The plan's `taskRef` resolves a key under
the registry's `tasks`, rather than a filename.

Neither file supplies `sourceBaseline`. That field is optional recorded-source
provenance in the current reader, so directly authoring a flow does not require
a recording. Earlier readers required it; an older APK may reject this example.
See the [flow reference guide](../flows/README.md) for that compatibility limit.

## Follow one run

The plan starts at `entryStep: summarize`. Its two nodes are:

```yaml
nodes:
  summarize:
    kind: task
    taskRef: summarize-text  # Key in the task registry.
    onVerified: done        # Explicit transition after output checks pass.
  done:
    kind: end
    outcome: succeeded
```

The registry's `summarize-text` task selects `model: coder`. Here, coder is a
model slot with no tool grants: `tools: []`. The plan also declares no MCP
servers or policies. The role name alone does not require a tool call. The
registry sets `loop: { maxSteps: 1 }`, bounding this task to one generation.

The registry resolves `prompt: prompts/minimal-summary.md` relative to its own
directory. That file asks for a short summary, without added facts or claims of
verification, then includes:

```text
Supplied text:
{{ user_input }}
```

The executor renders this binding with the caller's input before calling the
coder slot. **The plan's `input.promptTemplate` is reserved and inactive.**
Editing that field alone does not change the model's instructions. To change
what the model is asked to do, edit the registry-linked prompt file.

The task omits `completion`, so the current default is `model-prose`. Its output
contract declares `report: { from: model, type: prose }`. The executor binds the
returned prose under `summarize.outputs.report`, checks the declared output
structure, and follows `onVerified: done`. The end node supplies the succeeded
outcome; the model does not choose a transition by saying a special word.

`onVerified` does not mean the summary is factually verified. This example has
no independent evidence check. Its model output remains advisory prose, even
when its structure passes verification and the run succeeds.

## Budgets and model configuration

The plan declares `coder: { ctx: 4096 }`, a context requirement for the slot.
It does not name or include a GGUF file. In the test below, the caller supplies
a scripted model. A real Android run needs an app-configured coder model and
the app's normal model preparation.

The plan's `limits` declare budgets for model-generation steps (`maxSteps: 2`), reasoner calls
(`maxReasonerCalls: 0`), total tokens (`maxTokensTotal: 4096`), elapsed time
(`timeoutS: 60`) and artifact storage (`artifactMaxMb: 1`). These are run budgets,
separate from the task's one-generation loop bound. `onLimit: abort` selects the
terminal run-budget policy. These declarations do not measure summary quality.
The remaining `timeoutS` budget caps generation and tool-call deadlines, and the
runtime checks again before accepting a result or successful transition. A late
final generation cannot succeed. Native operations remain cooperatively bounded;
one in-flight load/decode cannot be preempted. See the architecture's implementation
qualifications.

## Run the existing checks

Use the repository root and an existing development environment. Prerequisites
are Python 3.11+ with the pinned packages in
[scripts/requirements.txt](../scripts/requirements.txt), and the Rust toolchain
pinned by [rust-toolchain.toml](../rust-toolchain.toml) with Cargo dependencies
available. These checks need no model weights, Android SDK/NDK or live MCP
server. Cargo may compile host code; check available RAM and disk first and use
bounded build concurrency. See [Contributing](../CONTRIBUTING.md) for environment
setup and broader checks.

```sh
free -h
df -h .
python3 scripts/validate-examples.py
CARGO_BUILD_JOBS=2 cargo test --locked -p trollm-core --test direct_authoring --no-default-features
```

[validate-examples.py](../scripts/validate-examples.py) checks execution-plan
schemas and transitions, then complete-reference task/subflow references and
prompt paths. It also reports procurement as a schema-only design sketch; that
report does not make procurement executable.

[direct_authoring.rs](../crates/trollm-core/tests/direct_authoring.rs) loads the
actual minimal-summary plan, registry and prompt and runs the real executor.
It supplies a `MockModel` with a fixed summary and a test gateway. It asserts:

- The plan and registry load without `sourceBaseline` and grant no tools.
- The caller's text reaches the rendered prompt in one model request.
- The run succeeds and `summarize.outputs.report` equals the scripted response.
- No tool intent or result events appear.

The same test target also exercises the shipped text-summary starter. Passing
these tests establishes source wiring and deterministic executor behavior with
scripted dependencies. It does not establish real inference, summary accuracy,
model performance, Android lifecycle behavior or live service acceptance.

For a small source exercise, change the prompt's wording, rerun the checks and
inspect how the test records model requests. The response stays fixed because
the model is scripted; evaluating the new instruction requires a real model.
There is no standalone flow-authoring runner supplied by these commands.

## After the tool-free example

[Ticket triage](../flows/examples/ticket-triage.flow.yaml) is the next engineering
reference. It adds MCP policies, artifact-backed worklists, `foreach` and
subflows, plus tool-free reasoner tasks and coder tool grants. Read its registry
and linked subflows through the [flow reference guide](../flows/README.md).
Its [conformance test](../crates/trollm-core/tests/conformance.rs) uses scripted
models and a test gateway. A real run needs a compatible tracker MCP service
and deployment-specific configuration; it is not the first runnable exercise.

These YAML files are source inputs. Installing a flow on Android requires the
existing hash-pinned bundle format; see the
[Android guide](../apps/android/README.md) and
[bundle reader](../crates/trollm-uniffi/src/bundle.rs). A loose plan file is not
an installable bundle.

For current details, consult the [execution-plan schema](../schemas/execution-plan.v1.schema.json),
[plan parser](../crates/trollm-core/src/plan.rs),
[registry reader](../crates/trollm-core/src/registry.rs),
[validator](../crates/trollm-core/src/validate.rs) and
[executor](../crates/trollm-core/src/exec.rs), alongside the implementation
qualifications and setup additions in the
[architecture](ARCHITECTURE-ANDROID.md). The
[authoring guide](FLOW-AUTHORING.md) and
[contract direction](FLOW-CONTRACT-DIRECTION.md) also discuss proposed work;
their proposed fields and tooling are not an implemented replacement for v1.
