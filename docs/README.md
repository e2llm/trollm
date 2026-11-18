# Learn trollm and choose a reading track

The teaching path builds a product picture before asking you to navigate runtime
contracts. Start with [Learn trollm](LEARN-TROLLM.md): one existing flow explains
what the models, tools and runtime each contribute. Then [Anatomy of a
flow](ANATOMY-OF-A-FLOW.md) uses a smaller example to make the actual source format
concrete. [Flow authoring](FLOW-AUTHORING.md) connects intent, task boundaries,
model evaluation and human review.

Each has a useful stopping point. You do not need the entire architecture or
roadmap to understand the product, and a small documentation correction does not
require reading every track.

To make a first change, use the
[starter prompt exercise](FLOW-AUTHORING.md#make-one-small-change-to-an-existing-starter).
It gives the exact source path, bundle command and scripted checks for an existing
starter. The longer authoring track below provides the wider design/evaluation
background when you need it.

For a fully specified ordinary operation that needs no model decision, see
[direct tool execution](DIRECT-TOOL-EXECUTION.md) and its zero-model calculation
example. This source capability is newer than the published 1.0.1 APK.

## Reading tracks

Costs describe complete assigned routes, not individual documents. They are
approximate full-text characters ÷ 4, rounded to 0.5k, measured for these routes
on 2026-11-11. Shared prerequisites and repeated files are counted once. The
additional column is for someone who has already read the product-orientation
route; totals and additional costs are rounded separately. Navigation, agent
boot context not listed in the route, command output,
and later task-specific code investigation are additional.

| Track and stopping outcome | Ordered route | Total | Additional after orientation |
| --- | --- | ---: | ---: |
| **Product orientation.** Explain the input-to-result journey, the roles, data bindings, current limits and optional surrounding services. | [Project overview](../README.md) → [Learn trollm](LEARN-TROLLM.md) | 4.0k | — |
| **First source flow.** Trace a real plan, registry and prompt into the existing scripted test, and explain what that test does and does not establish. | Orientation → [Anatomy](ANATOMY-OF-A-FLOW.md) → its [plan](../flows/examples/minimal-summary.flow.yaml), [registry](../flows/tasks/minimal-summary.tasks.yaml), [prompt](../flows/tasks/prompts/minimal-summary.md) and [test](../crates/trollm-core/tests/direct_authoring.rs) | 7.5k | 3.0k |
| **Flow design and model evaluation.** Specify focused tasks, evidence handoffs, expected alternatives and a useful evaluation plan using current examples. | Orientation → Anatomy → [authoring](FLOW-AUTHORING.md) → [practices](FLOW-PRACTICES.md) → [prompting](FLOW-PROMPTING.md) → [flow references and limits](../flows/README.md) | 22.5k | 18.5k |
| **Control and repeated work.** Explain authored routes, child/iteration outcomes, evidence destinations and recovery limits before changing their implementation. | Orientation → Anatomy → [adaptive flows](ADAPTIVE-FLOWS.md) → [reliability](FLOW-RELIABILITY.md) → [task result artifacts](TASK-RESULT-ARTIFACTS.md) → [child-return contract](amazon-generalization/G2-CONTRACT.md) | 14.0k | 10.0k |
| **Android lifecycle and acceptance.** Distinguish source, installed bundle, model assets and APK identity; identify the preparation/control and device checks a result needs. | Orientation → [Android guide](../apps/android/README.md) → [model preparation](MODEL-PREPARATION.md) → [versioning](VERSIONING.md) | 12.0k | 8.0k |
| **Contributor orientation.** Find the responsible module, applicable contract/checks and the correct contribution path without treating a proposal as implemented behavior. | Orientation → [AGENTS](../AGENTS.md) → [Contributing](../CONTRIBUTING.md) → [Premise](PREMISE.md) | 13.0k | 8.5k |

Read the union when work crosses tracks rather than adding their totals. The
assigned files above are shared by the source and curated checkouts; roadmap and
operator history can differ and are not hidden prerequisites in these budgets.
Refresh estimates when the assigned route or its material changes substantially.

Finishing an implementation-oriented reading track identifies the contracts to
preserve. It does not replace reading the affected schema, reader, implementation
and tests at the actual revision in use. Those lookups depend on the feature.
The [architecture](ARCHITECTURE-ANDROID.md) is the deeper reference, including
later implementation qualifications; it is not a universal first read.

## Choose the relevant contract

| Working on… | Read for this question |
| --- | --- |
| Product motivation, role boundaries or a proposed extension | [Premise](PREMISE.md) for rationale; [current architecture](ARCHITECTURE-ANDROID.md) and actual readers for supported behavior |
| Task design, authoring inputs or human feedback | [Flow authoring](FLOW-AUTHORING.md); its current method and proposed complete authoring pack are distinguished |
| Ambiguity, prompt wording or model configuration | [Flow prompting](FLOW-PROMPTING.md): compare actual decisions, appropriate and unnecessary declines, scope adherence and useful coverage |
| Flow control, per-step context or generation bounds | [Adaptive flows](ADAPTIVE-FLOWS.md), its schemas and executable routing example |
| Repeated child calls, child outcome exits or original-record selection | [Flow reliability](FLOW-RELIABILITY.md): frame-owned iteration and retained evidence; the read-only replay helper is not full-mission acceptance |
| Computation or record filtering/selection/ordering | [Built-in tools](BUILTIN-TOOLS.md): exact operations and their limits; correct membership or arithmetic does not prove semantic suitability |
| Preserving different native reads of the same remote method | [Task-specific result artifacts](TASK-RESULT-ARTIFACTS.md): destination names, replacement and cache boundaries |
| Worklists, child exports, bounded parent views or metrics | [Collection contracts](amazon-generalization/README.md), [child returns](amazon-generalization/G2-CONTRACT.md) and [measurement boundaries](amazon-generalization/PERFORMANCE.md) |
| Android setup, models, credentials or worker lifecycle | [Android guide](../apps/android/README.md) and [model preparation](MODEL-PREPARATION.md) |
| Moray or another source integration | [Owned source connector](MORAY-SOURCE-CONNECTOR.md) and [native collection offer](NATIVE-COLLECTION-MEMBERS.md); source/member checks are distinct from installed admission and outcome evidence |
| Human package inspection and feedback | [Flow review companion](../apps/flow-review/README.md), with its [design and interface](flow-review/README.md); the viewer does not execute a flow |
| Proposed catalogue or broader authoring format | [Flow contract direction](FLOW-CONTRACT-DIRECTION.md) and the [proposed authoring pack](FLOW-AUTHORING.md#supply-the-authoring-pack); proposed fields may not exist in the runtime |
| Source or APK release | [Versioning](VERSIONING.md), [third-party notices](../THIRD_PARTY_NOTICES.md) and the contribution workflow for the checkout |
| Security or private reporting | [Security policy](../SECURITY.md) and [enterprise security](SECURITY-ENTERPRISE.md) |

## Status and evidence

The [project overview](../README.md#current-evidence-and-limits) holds the concise
acceptance boundary; the [product plan](PRODUCT-PLAN.md) and live issues hold
priorities. The development plan includes dated records, while the curated
checkout carries its public roadmap. Check the date and subject before treating
an old build, device result or implementation proposal as current acceptance.

Keep four questions separate: does the source validate, does the runtime enforce
its declared contract, do the selected models interpret the task correctly, and
does the actual run establish the desired outcome? Scripted/stub tests do not
answer all four. Neither a new source commit nor a passing fixture establishes
acceptance of a current APK on a physical phone.

When contract and implementation disagree, surface the difference and resolve it
with the maintainer. Newer code can still violate an intended contract; a future
proposal does not silently change today's parser. Use [Contributing](../CONTRIBUTING.md)
for the source/public-review path and the checks for the changed layer.
