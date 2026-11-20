# trollm — Premise

## What this is

trollm is a runtime for orchestrating **small on-device models** on Android with a
**deterministic execution plan** ("the ledger") between them:

- **Coder model** — code/JSON/tool-call-tuned (e.g. Qwen3.5-0.8B class). The only model allowed
  to touch tools (MCP). Runs in loops; each step's result feeds the next.
- **Reasoner model** — general reasoning. Advisory only. Receives raw
  info + a question, replies in **free prose** ("close this", "no option is good"), never
  parsed by the ledger.
- **Ledger** — not a model. An interpreter that loads a versioned execution plan, drives the
  two models through it, mediates all file/artifact I/O, proxies all MCP traffic with smart
  policies (cache / paginate / shape / mutation journaling), enforces budgets, and records
  every transition in an append-only event log.

Coder and reasoner are the two primary roles; a flow need not use both. This page
explains the design rationale and later extension ideas. Start with
[Learn trollm](LEARN-TROLLM.md) for a complete worked product picture and
[Anatomy of a flow](ANATOMY-OF-A-FLOW.md) for the current format. Runtime contracts
and their implementation qualifications take precedence over a future extension
sketch below.

The current browser starter pins a Qwen3.5-0.8B Q4_K_M coder and a Gemma 4 E2B IT
Q4_K_M reasoner, both used for text. Its [setup descriptor](../flows/starters/browser-summary/setup.json)
records the exact model files, revisions, checksums and settings. The
[minimal summary](ANATOMY-OF-A-FLOW.md) uses only the coder slot.
The runtime's explicit transition rules determine progress through the authored
flow. See the [product plan](PRODUCT-PLAN.md) for the application experience and
the [flow ICD direction](FLOW-CONTRACT-DIRECTION.md) for planned format changes.

## What users should be able to do

The primary product path is a supplied workflow: choose a flow, configure its
model assets and applicable connections, provide input, then Start and follow
progress and result.
A browser flow may require E2LLM sign-in; a local calculator flow does not require
that connection. Required downloads and setup precede execution; native model
preparation follows an explicit Start/Resume under the selected flow's limits.
Browser actions can take place on a connected PC while inference runs on the
phone. This is the intended experience; the integrated Android app is still a
prototype.

A flow is more generally a procedure combining MCP tools, prompts, explicit
control flow and persistent artifacts. Examples include location → nearby-place
enrichment → recommendations, sensors → reasoning → database query → action,
and the batch-processing references below. Moray is a separately planned
flow-authoring integration, not a runtime dependency; direct LLM-assisted
authoring from the same specification must work without it.

Local inference makes the phone the model runtime and avoids requiring a cloud
model API for each reasoning step. Remote MCP tools can still transmit data,
require connectivity and have their own service costs. Offline operation depends
on the connections and assets used by the particular flow.

## Why this runtime

The design gives sequencing, verification, budgets and recovery explicit runtime
rules and uses the models for bounded reasoning and tool selection. The target
is a small Android application with durable execution and a controlled native
dependency footprint. This is a project-specific design choice; comparisons
with other orchestration frameworks need to distinguish their supported modes
and actual deployment requirements.

## Design invariants

These are load-bearing. Any change that violates one is a redesign, not a patch.

1. **The graph defines allowed flow control; the runtime enforces it.** Transitions
   use verification results, enumeration state, budgets and explicitly declared
   coder tool choices. The [adaptive-flow contract](ADAPTIVE-FLOWS.md)
   adds bounded selection among authored exits. Model prose never becomes a
   branch condition, and models cannot invent graph edges. No magic strings.
2. **The runtime retains and binds structured data; advice stays prose.** Worklists,
   item records and write intents move through artifacts and bindings. Known
   values should not require model transcription. A coder can select model-owned
   tool arguments through the decode-constrained, validated tool-call boundary;
   the calculator example needs such a choice. That exception does not make
   prose advice an executable structure or let a model replace the original records.
3. **Schema-free inter-model communication.** The reasoner replies in free text. Its output
   is injected verbatim into the coder's context. The ledger observes *behavior* (tool calls)
   not *intent* (prose).
4. **Everything is content-addressed and pinned.** Flows, task registries, model weights, and
   source baselines are SHA-256 addressed. A pinned flow referencing a mutable task is not
   pinned.
5. **Persisted execution state governs continuation.** The run has an append-only
   journal, snapshots and artifacts. Checkpoints preserve progress at node
   boundaries; recovery also validates journal watermarks and uncertain/completed
   writes beyond the saved snapshot. A journal file alone is not a complete
   external-world snapshot or permission to repeat an action. Use the current
   architecture's recovery rules rather than replaying calls from a transcript.
6. **Lean footprint.** No broker daemons on-device. Target < 150 MB RSS for
   orchestration outside the model runtime. Measure the configured model pair's
   memory separately on the target phone; the earlier two-1B-model RAM example
   is not a measurement of the current model targets.
7. **Authored escalation.** Reasoner consultations and permitted alternatives are
   declared in the flow. Coder control tools can select declared exits under
   runtime budgets; they cannot create consultations or arbitrary destinations.

## Use cases

The first user-facing showcase is an E2LLM browser flow. Ticket triage is the
current complete engineering reference with scripted test dependencies;
procurement is a design sketch. See the [flow guide](../flows/README.md).

### Engineering reference: ticket triage

MCP fetches an issue list. The smart proxy delivers page 1 to the coder and background-fetches
the full listing into a local file. The ledger binds the worklist **from the artifact file**
(not from model output) and iterates it (`foreach`, snapshot enumeration). Per item: coder
enriches via `get_issue(id)` — served by the proxy **from the cached file**, not the network —
then the reasoner is consulted ("what to do about it", free prose: close / update / comment /
nothing), then the coder infers and executes the corresponding MCP **write** (mutation class:
passthrough, intent-logged for crash-safe resume). Batch finalize summarizes from the event
log. `maxItems` → `paused` composes with Android backgrounding; resume-from-cursor is the
expected path.

### Procurement ("buying a list of products")

Same skeleton: MCP catalog search (paginated → cached), per-product enrichment from cache,
reasoner applies user constraints in prose ("this one violates the allergy constraint"),
coder executes cart/order mutations. Demonstrates the pattern is generic: *paginated list →
snapshot iteration → cached enrichment → advisory prose → inferred mutation*.

### Other instances of the same pattern

- Log/alert batch review (enrich from local cache, reasoner triages, coder files tickets)
- Document processing pipelines (per-chunk analysis with steering)
- Home-automation batch ops (device list → per-device checks → advisory → actuation)

## Runtime structure

| Layer | Choice | Rationale |
|---|---|---|
| Ledger core | Rust crate, pure Rust API | workflow execution is separate from the Android UI; UniFFI connects Rust and Kotlin |
| Execution plan | YAML, `task`/`foreach`/`end` node kinds, transition keys, `flowRef` stack | matches how flows are actually authored; no generic graph walker |
| Model runtime | llama.cpp (GGUF), GBNF on the tool-call fence only | models supported by the pinned backend; constrained decode for tool-call syntax |
| MCP | in-process smart proxy (streamable HTTP client + policy engine) | single choke point; cache/paginate/shape/mutation policies per tool |
| Event log | JSONL per run | audit + replay + resume + queue in one artifact |
| Artifacts | content-addressed files + meta sidecars | atomic writes (tmp+rename), completeness flags |
| Android app | Kotlin/Compose UI + foreground service | OS-mandated lifecycle |

## Design reasons

The runtime uses one native interpreter inside the Android app. Model calls,
tool policies and saved evidence share explicit contracts without requiring a
separate orchestration service on the phone. Platform lifecycle and credential
handling remain with the app.

Model advice stays in prose. Tool-call syntax is constrained and validated before
dispatch, while saved structured data passes through bindings. This avoids asking
a model to transcribe an existing worklist or turning an unvalidated phrase into
a control signal. These mechanisms constrain execution; they do not establish
that a model chose the right product, record or operands.

## Model and backend limits

Current [model preparation](MODEL-PREPARATION.md) overlaps up to two referenced
slots after Start/Resume and uses lazy loading when a plan references more than
two. Available memory and the selected model files still limit practical use.

Any alternative model backend must preserve the `ModelSlot` generation contract,
including the coder's grammar constraints, limits and failure behavior. A backend
or model supported elsewhere is not automatically supported by this app.

## Security scope

[Enterprise security](SECURITY-ENTERPRISE.md) distinguishes current controls from
deferred features such as signed publisher identities, stronger audit integrity
and installation policy. The current app does not claim those deferred controls.
