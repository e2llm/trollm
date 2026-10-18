# Flow interface contract: direction and authoring brief

Status: **design direction, 2026-10-11**. This is the brief for action F1 in the
[product plan](PRODUCT-PLAN.md), not a new parser format or an implemented schema.
The current [v1 schema](../schemas/execution-plan.v1.schema.json),
[plan model](../crates/trollm-core/src/plan.rs) and
[bundle importer](../crates/trollm-uniffi/src/bundle.rs) remain the implementation
reference until a versioned change lands with its consumers.

## Purpose and ownership

Specify enough for a flow producer to deliver a runnable procedure without
knowing trollm's implementation. Jointly author the interface control document
(ICD) with contributors to Moray, an optional browser-flow authoring project.
Moray exports the contract;
trollm consumes it. Direct human/LLM authoring is another first-class producer.

The contract must be self-contained. Generic flows need no Moray installation,
account, browser recording or Moray-specific identifiers. Recorded baselines and
their derivation evidence can be optional provenance for flows that have them.
An authoring LLM receives the spec, tool definitions and requirements; it need
not inspect either project's source.

| Term | Meaning |
| --- | --- |
| Flow | A versioned executable procedure with declared inputs, steps, dependencies, prompts and results |
| Execution plan | The graph/control-and-data portion interpreted by the runtime |
| Flow bundle | The installable flow plus registries, prompts, subflows and metadata |
| Run | One execution with concrete inputs, selected configuration, events and artifacts |
| Artifact | Stored working data or output, with an identity and the completion/provenance information needed by the executor |

## Information the contract must cover

| Area | Required meaning |
| --- | --- |
| Identity and packaging | Flow ID/version, format version, entry point, included files/hashes and optional author/source provenance |
| Presentation | Title, purpose, readable step labels, starting input fields/defaults, declared user-facing result and connection requirements |
| Models | Logical roles, default model sources/files and runtime configuration; Hugging Face repository/revision/file and hashes where used |
| MCP connections | Logical names, default endpoints, auth method/configuration, referenced tools and their invocation contracts; credentials resolved by the app |
| Control flow | Task sequencing, supported transitions, iteration/subflows, limits and error outcomes with unambiguous semantics |
| Data flow | Run inputs, scoped outputs, artifact references, tool arguments and final results; what a task can read and produce |
| Execution state | Completion evidence, attempts, progress, pause/resume/cancel and recovery after interrupted external actions |
| Diagnostics | Stable run/step/attempt identity, effective configuration references, causes and relevant outputs/artifacts |
| Compatibility | Explicit format evolution and migration rules; unsupported formats produce clear errors |

Exact field names and placement are work for F1. Keep the executable graph's
references to logical roles/connections, with acquisition and UI information in
bundle metadata. This makes the flow the source of its setup while avoiding
repetition across individual nodes.

## Default configuration and overrides

The app uses the flow's supplied defaults and honors the user's overrides.
There is **no advance model/MCP compatibility check**, approved-preset matrix or
confirmation gate for an expert override. If a selected model fails to load or
a tool call fails, report the actual operation, configuration and error.

A proposed resolution order is published flow defaults → saved flow overrides
→ explicit run choices. Record the selected flow version, model identities and
non-secret settings with each run. Specify which changes affect future runs and
how resumed runs retain their original bindings. Credentials remain references
to app-managed accounts; normal token refresh does not require a new flow.

Model downloads should stream, show progress/size, reuse installed files and
recover from interruption. Choose a clear UI trigger—installation, enabling or
first run—and allow optional prefetch. MCP configuration may describe OAuth or
Bearer setup, including public client/endpoint settings where needed; tokens,
passwords and private client secrets are never bundle content.

Bundle parsing, file integrity checks and runtime limits remain ordinary
correctness behavior. Authoring checks and release tests exercise the contract;
they do not predict whether every user-selected configuration will work.

## Execution semantics to preserve or specify

- Models perform bounded tasks; explicit runtime rules determine progress.
  Preserve the current reasoner's advisory prose contract. Typed tool artifacts
  and UI records do not imply a new requirement for free-form model prose to
  become structured control instructions.
- Declare task inputs, outputs and completion conditions. Distinguish a model's
  assertion, a tool observation and verified completion. Unknown or stale
  evidence needs a defined outcome.
- Preserve item identity and scope during iteration. For N successful items,
  define setup, item work, between-item work and finalization explicitly;
  between-item work runs max(N−1, 0) times. A retry stays on the same item.
- Define zero-item and limit outcomes. Partial work, missing enumeration and
  failures must not silently become successful completion.
- Persist outputs, progress and operation identities. Reconstructing state must
  not repeat completed model or tool operations. An uncertain external action
  needs reconciliation before an unsafe retry; a durable cursor alone does not
  establish exactly-once side effects.
- Define user-facing outputs in addition to intermediate artifacts. Begin with
  readable text and references to factual results. Rich cards can be added with
  explicit bindings rather than inferred from arbitrary prose.

Use the existing task/foreach/end primitives where sufficient. Additional choice,
call/return or polling primitives require demonstrated examples and specified
consumer behavior. Parallel browser control needs its own resource semantics
and is outside the initial authoring contract.

These semantics draw on the Moray graph-execution proposal, which is itself a
proposal rather than a ratified shared v2 format. Reuse the reasoning and agree
the interface; do not assume the existing schemas are interchangeable.

## Runtime diagnostics

A concise UI error should name the failing step and what happened. Its details
should provide enough information for a person or authoring LLM to reproduce or
repair it:

- App/build, flow/bundle version and run/step/attempt identifiers.
- Model identity and effective inference settings, or connection/tool name and
  endpoint identity relevant to the failure.
- Error chain and protocol status, plus relevant redacted arguments, tool
  responses, model output or artifact references.
- Last verified completion and whether an external action's outcome is known.
- What happened to the run: failed, paused, cancelled, or resumable at a stated
  point. Do not substitute a generic “something went wrong” message.

Keep technical details expandable in the app and exportable by the user.
Credential redaction must include auth headers and secret-bearing URLs. Export
only relevant data, and identify artifacts that contain the user's content.
This is local troubleshooting; it introduces no automatic telemetry service.

## Gaps from the current implementation

| Current behavior | Change to design and implement |
| --- | --- |
| Plan model slots declare context only; weights are app-owned | Bundle metadata supplies model references/defaults and the app resolves them to logical slots |
| MCP plan entries carry name and URL | Describe account/auth/configuration defaults and UI overrides without embedding credentials |
| The input declaration contains a prompt template | Add a well-defined starting-input form and binding contract |
| Manifest contains entry flow, task registry and file hashes | Version dependency/presentation metadata and define its identity and compatibility behavior |
| Source baseline is optional in the current v1 reader | Directly authored flows can omit it; existing recorded-source objects remain supported |
| Registry/plan outputs serve the executor | Define which outputs are presented to users and how presentation refers to them |
| No standalone production packer | Generate complete bundles and hashes from documented authoring inputs |

The first implementation slice relaxes the v1 provenance requirement without
introducing new metadata or changing bundle identity. Older readers still
require `sourceBaseline`; the [minimal summary](../flows/README.md) needs the
updated reader. Full F1 remains open.

Do not insert proposed fields into today's v1 examples and imply they already
work. F1 must include the schema, parser/importer, bindings and authoring-example
changes together, plus an explicit existing-bundle compatibility decision.
Metadata and dependency changes must also affect bundle/run identity appropriately;
the current importer keys installation by the root flow hash.

## Joint specification work package

1. Write two minimal use cases: one supplied E2LLM browser flow and one directly
   authored location/enrichment/recommendations flow. Name the actual tool
   contracts and leave unavailable providers explicitly unresolved.
2. Map their needs to existing plan, registry, manifest and app configuration
   fields. Keep unsupported future features out of the first schema.
3. Choose format/version identifiers, metadata placement, override precedence,
   result bindings, identity and migration behavior.
4. Publish the ICD, schema, complete fixtures and the
   [LLM authoring pack](FLOW-AUTHORING.md). The spec must be usable on its own.
5. Implement/export/import against the same version and record cross-producer
   conformance examples. Exercise empty input, failed tools, interrupted runs
   and malformed content alongside the successful examples.

The execution principles needed by this proposal are summarized above;
the contract must remain usable without access to another project’s source.
