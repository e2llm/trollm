# trollm — Premise

## What this is

trollm is a lean runtime for orchestrating **two small on-device models** on edge
hardware (Android phone first; Raspberry Pi / Linux edge boxes later) with a
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

The current browser starter pins a Qwen3.5-0.8B Q4_K_M coder and a Gemma 4 E2B IT
Q4_K_M reasoner, both used for text. Its [setup descriptor](../flows/starters/browser-summary/setup.json)
records the exact model files, revisions, checksums and settings. The
[minimal summary](ANATOMY-OF-A-FLOW.md) uses only the coder slot.
The runtime's explicit transition rules determine progress through the authored
flow. See the [product plan](PRODUCT-PLAN.md) for the application experience and
the [flow ICD direction](FLOW-CONTRACT-DIRECTION.md) for planned format changes.

## What users should be able to do

The primary product path is a supplied workflow: install the APK, sign in to
E2LLM, choose a flow, provide any starting input and follow its progress and
result. The app prepares the models and connections declared by that flow.
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
   coder tool choices. The approved [adaptive-flow contract](ADAPTIVE-FLOWS.md)
   adds bounded selection among authored exits. Model prose never becomes a
   branch condition, and models cannot invent graph edges. No magic strings.
2. **The ledger handles all structured data; models handle only prose.** Structured data
   (worklists, item records, write intents) moves ledger-to-ledger (artifact files → variable
   bindings). Prose moves model-to-model. Models never transcribe data; the ledger never
   parses prose. The single exception, by necessity: the coder's tool-call fence, which is
   decode-constrained (GBNF) and validated post-hoc.
3. **Schema-free inter-model communication.** The reasoner replies in free text. Its output
   is injected verbatim into the coder's context. The ledger observes *behavior* (tool calls)
   not *intent* (prose).
4. **Everything is content-addressed and pinned.** Flows, task registries, model weights, and
   source baselines are SHA-256 addressed. A pinned flow referencing a mutable task is not
   pinned.
5. **The event log is the ledger.** Append-only JSONL per run. It is simultaneously the audit
   trail, the replay mechanism, the pause/resume state, and (where pull semantics are needed)
   the queue. State transitions happen only at node boundaries, so resume never splits a
   model call or a tool call.
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

## Proposed approach (summary)

| Layer | Choice | Rationale |
|---|---|---|
| Ledger core | Rust crate, pure Rust API | shared with Pi/desktop later; UniFFI → Kotlin |
| Execution plan | YAML, `task`/`foreach`/`end` node kinds, transition keys, `flowRef` stack | matches how flows are actually authored; no generic graph walker |
| Model runtime | llama.cpp (GGUF), GBNF on the tool-call fence only | arbitrary models; constrained decode where it matters |
| MCP | in-process smart proxy (streamable HTTP client + policy engine) | single choke point; cache/paginate/shape/mutation policies per tool |
| Event log | JSONL per run | audit + replay + resume + queue in one artifact |
| Artifacts | content-addressed files + meta sidecars | atomic writes (tmp+rename), completeness flags |
| Android app | Kotlin/Compose UI + foreground service | OS-mandated lifecycle |

## Rejected approaches (and why)

| Rejected | Why |
|---|---|
| Adopting a general orchestration framework as the runtime | The project currently chooses a small native Android interpreter with explicit model/tool/artifact contracts; any alternative needs a concrete footprint and integration comparison |
| MCP gateways (IBM ContextForge, MCPJungle, Docker MCP GW, Lunar.dev, Obot) | Registries, auth stacks, multi-tenant governance — the n8n problem one layer down. One device, one client: in-process middleware wins |
| A2A protocol | JSON-RPC over HTTP for cross-org delegation. On one device a function call beats a protocol; adds bytes, buys nothing |
| dora-rs **on Android** | Daemon/process model fights the app sandbox; Zenoh SHM broken on Android (bionic lacks `shm_open`); its value (multi-process, multi-machine, hot reload) is muted in a single app. Kept as the Pi/multi-device extension path |
| gearmand **on-device** | Same daemon-in-sandbox problem; the ledger's event log already provides pull-queue semantics. Kept for cross-machine durable dispatch (SQLite/Redis backends) |
| ROS 2 | Massive overkill; relevant only as interop target via zenoh-bridge-dds |
| Zenoh standalone on a single device | A component with no job. Arrives free with dora when multi-device happens |
| LiteRT-LM / ExecuTorch as primary runtime | Constrained generation for arbitrary GGUFs is the blocker (coder needs GBNF). Kept as future NPU-offload adapters |
| Magic strings / strict schemas between models | Sub-1B models are unreliable at strict outputs; prose between models, structure only at the tool-call fence |
| Raw multicast + UDP sockets for device fabric | Rebuild discovery, topics, fragmentation, reliability, NAT traversal badly. Zenoh already is that, done well |
| Worklist/items sourced from model output | Sub-1B transcription corrupts lists. Structured data binds from artifacts |

## Extensibility paths (not built now, design must not foreclose)

- **Pi / Linux edge**: dora-rs wraps ledger-core nodes as a dataflow graph; nodes as processes;
  Zenoh pub-sub for cross-machine; gearmand (SQLite or Redis queue backend) for durable
  cross-machine job dispatch where pull semantics are needed.
- **Multi-device**: Zenoh arrives via dora — NAT traversal, discovery, location-transparent
  key expressions; Zenoh storage/queryables can turn the per-device MCP cache into a shared
  cache. dora's service/action patterns map onto ledger consults.
- **More/larger models**: llama.cpp router mode or llama-swap for >RAM model sets; model slot
  abstraction already assumes exactly two resident slots but nothing else.
- **NPU offload**: LiteRT-LM (Snapdragon/Dimensity NPU) or ExecuTorch (ANE/NNAPI) adapters
  behind the same `ModelSlot` trait — blocked on their constrained-decoding support for the
  coder slot.
- **WasmEdge/LlamaEdge**: if hard cross-platform portability of the whole stack becomes a
  requirement (runtime + API server < 30 MB).
- **ROS 2 interop**: zenoh-bridge-dds, only if robotics hardware enters the picture.

## Enterprise tech debt (see docs/SECURITY-ENTERPRISE.md)

On-prem airgap deployment, enterprise-grade security, and auditability are **requirements,
not options** — but parts are deliberately deferred. The architecture must not foreclose:
signed content bundles, hash-chained audit logs, mTLS with internal CA, egress allowlists,
Keystore/HSM secrets, RBAC on flow installation, SBOM, offline update channel. What is
designed-in now vs deferred is tracked in the security doc.
