# trollm — Android Architecture (v1 target)

Normative document. Implementation agents: **implement exactly this**. If a consequential
decision is missing or contradictory here, STOP and report the question — do not assume.
Non-consequential choices (formatting, internal helpers, which micro-library for YAML) are
yours to make.

The current prototype constraints in §16 qualify the earlier target behavior where
the baseline review found unsafe or incomplete implementations.

The [product plan](PRODUCT-PLAN.md) records the next product direction. Its
[flow ICD brief](FLOW-CONTRACT-DIRECTION.md) proposes flow-owned model/MCP
defaults and UI metadata. Those changes need a versioned schema/consumer
migration; they are not fields accepted by this document's current v1 format.
Optional recorded-baseline provenance is supported by the current v1 reader as
described below. In particular, the app-only weight-selection
rule below describes v1 rather than the intended future bundle setup experience.

## 1. System overview

```
┌─ Android app (Kotlin/Compose) ────────────────────────────────┐
│  UI: flow picker, user input, run progress (tails event log), │
│      final response, pause/resume/cancel                      │
│  Foreground service + partial wake lock during runs           │
│  └─ UniFFI bindings ──► trollm-core (Rust)                │
│        ├─ Plan loader/validator (signed, sha256-pinned)       │
│        ├─ Executor: stack machine over task/foreach/end       │
│        ├─ Event log: runs/<uuid>/events.jsonl                 │
│        ├─ Artifact store: runs/<uuid>/artifacts/              │
│        ├─ Budgets: counters from plan limits                  │
│        └─ RunState snapshot/restore (pause/resume/crash)      │
│  ├─ trollm-models (Rust)                                  │
│  │    ├─ ModelSlot trait                                      │
│  │    ├─ coder slot: llama.cpp GGUF, GBNF on tool-call fence  │
│  │    └─ reasoner slot: llama.cpp GGUF, free prose            │
│  └─ trollm-mcp (Rust)                                     │
│       ├─ MCP client: streamable HTTP to remote server(s)      │
│       └─ Policy engine: cache / paginate / serve_from /       │
│          shape / mutation(intent-log)                         │
└───────────────────────────────────────────────────────────────┘
         HTTPS (mTLS later)              local files only
                  │                            │
         remote MCP server(s)      models/*.gguf (pinned sha256)
```

## 2. Repository layout

```
trollm/
├── docs/                          # this doc, PREMISE.md, SECURITY-ENTERPRISE.md
├── schemas/execution-plan.v1.schema.json
├── flows/
│   ├── examples/ticket-triage.flow.yaml
│   ├── examples/procurement.flow.yaml
│   └── tasks/tickets.tasks.yaml   # task registry example
├── crates/
│   ├── trollm-core/           # plan model, validator, executor, log, artifacts, budgets
│   ├── trollm-models/         # ModelSlot trait + mock + llama.cpp adapter (feature-gated)
│   ├── trollm-mcp/            # MCP client + policy engine
│   └── trollm-uniffi/         # UniFFI wrapper (separate crate; core stays FFI-agnostic)
├── apps/android/                  # Kotlin/Compose app
└── Cargo.toml                     # workspace
```

## 3. Execution plan format (ledger.execution-plan.v1) — NORMATIVE

```yaml
schemaVersion: ledger.execution-plan.v1
id: ticket-triage
version: 3
sourceBaseline: { sha256: "..." }          # optional recorded-source provenance

input:
  promptTemplate: "..."                     # may reference {{ user_input }}

limits:                                     # run-level budgets, ledger-enforced
  maxSteps: 24
  maxReasonerCalls: 3
  maxTokensTotal: 60000
  timeoutS: 300
  artifactMaxMb: 50
onLimit: respond_partial                    # terminal policy: abort | respond_partial

models:                                     # SLOTS ONLY. Weights are pinned by the app,
  coder: { ctx: 8192 }                      # never by the plan. Plan may not name weights.
  reasoner: { ctx: 4096 }

mcp:
  servers:
    - { name: tracker, url: "https://mcp.example.com" }   # auth resolved by app, not plan
  policies:                                 # see §6
    list_issues: { paginate: {...}, cache: {...} }

entryStep: setup
nodes:
  setup:
    kind: task
    taskRef: prepare-worklist
    outputs:
      items: { fromArtifact: list_issues.full_fetch, format: jsonl }   # NOT from model
    verify: { artifactComplete: list_issues.full_fetch }               # §4.3
    onVerified: each
    onError: { retry: 2, backoffS: 5, then: fail-run }                 # §4.5

  each:
    kind: foreach
    items: { from: setup.outputs.items }
    itemKey: id
    bindAs: item
    enumeration: snapshot                  # v1: only "snapshot" is legal
    ordering: source                       # v1: only "source" is legal (sequential)
    maxItems: 500
    onLimit: { status: paused, reason: item-limit }
    body:    { flowRef: process-item, inputs: { item: { from: item } } }
    between: { flowRef: return-to-list }   # optional; runs after each item
    outputs: { results: { collect: body.outputs, indexBy: itemKey } }  # §4.6
    onEmpty: finish-empty
    onComplete: finish

  finish:
    kind: task
    taskRef: finalize-batch
    onVerified: done
  finish-empty:
    kind: task
    taskRef: finalize-empty-batch
    onVerified: done
  done:  { kind: end, outcome: succeeded }
  fail-run: { kind: end, outcome: failed }
```

`sourceBaseline` may be omitted for a directly authored flow. If present, it
retains the existing `{sha256: ...}` shape; use a real source digest rather than
inventing provenance. Explicit `null` is invalid. This reader relaxation keeps
`ledger.execution-plan.v1` and the bundle format unchanged and continues to read
existing plans. Earlier readers require the field and cannot load a flow that
omits it; use an updated build for the minimal summary example. File pins and
installed-flow identity still hash the original YAML bytes, including any
provenance that is present.

### 4.1 Node kinds — closed set

`task` | `foreach` | `end`. Nothing else in v1. A reasoner consult is a `task` whose
taskRef binds the reasoner slot and grants no tools.

### 4.2 Transition keys — closed set

`onVerified`, `onComplete`, `onEmpty`, `onLimit`, `onError`. Values are node ids, or inline
`{ status: paused|failed, reason: string }`. **No expressions, no conditions on model
output.** A missing transition key for an outcome that occurred = plan validation error at
load time where statically checkable, else runtime `failed` with reason `unhandled-transition`.

### 4.3 Verification (ledger-side, deterministic)

Per task, the registry declares an output contract. Verification checks **structure only**:
required outputs exist, types match, artifact references exist and (for `fromArtifact`
outputs) the meta sidecar says `complete: true`. Verification NEVER inspects model prose.
A `task` whose contract accepts a no-op must say so (`allowNoop: true`) — triage's `act`
step is the canonical case.

### 4.4 Executor = stack machine

- `flowRef` pushes a call frame (plan id + sha256, bound inputs, own counter scope).
- `foreach` executes: snapshot `items` once → per item push body frame → after each item,
  push `between` frame → aggregate per §4.6.
- Run state (serializable at every node boundary):

```json
{
  "runId": "uuid", "status": "running|paused|succeeded|failed",
  "stack": [ { "plan": "ticket-triage@sha256:...", "node": "each",
               "vars": { "...": "..." } } ],
  "foreachCursors": { "each": { "index": 42, "snapshotRef": "artifacts/....jsonl" } },
  "counters": { "steps": 118, "reasonerCalls": 2, "tokens": 41000, "startedAt": "..." }
}
```

- Pause/resume: `paused` state is persisted; resume = load state, continue dispatch.
  Crash recovery: identical path (resume from last persisted boundary; at most one node
  is re-executed — see §6 mutation journaling for write safety).
- Counters persist across pause/resume. Per-node budgets (e.g. `maxSteps` on a coder-loop
  task) reset on node entry; run-level budgets never reset.

### 4.5 onError

Every `task` may declare `onError: { retry: N, backoffS: S, then: <nodeId|{status...}> }`.
Default if absent: `retry: 0`, then `{ status: failed, reason: <error-class> }`.
Error classes (closed enum): `mcp-unreachable`, `mcp-error`, `tool-call-parse`,
`verification-failed`, `artifact-io`, `budget-exhausted`, `model-error`.

### 4.6 foreach output aggregation

Each body iteration's declared outputs append to `<foreachId>.outputs.results[]`, indexed by
`itemKey`. Partial results persist across `paused`. `onEmpty` fires when snapshot length is 0.

## 5. Tasks and the registry (ledger.task-registry.v1)

Plans name `taskRef`s; the registry (separate YAML, also sha256-pinned) defines them:

```yaml
schemaVersion: ledger.task-registry.v1
tasks:
  prepare-worklist:
    model: coder                        # slot name only
    prompt: prompts/prepare-worklist.md # template; may reference inputs, user_input
    tools: [tracker.list_issues]        # allowlist; may be empty
    loop: { maxSteps: 8 }               # coder-loop semantics; omit for single-shot
    outputContract:
      outputs:
        items: { fromArtifact: list_issues.full_fetch, format: jsonl, type: list }
      requireArtifactComplete: [list_issues.full_fetch]
  consult-steering:
    model: reasoner
    prompt: prompts/consult-steering.md
    tools: []                           # reasoner never gets tools
    loop: { maxSteps: 1 }
    outputContract:
      outputs: { advice: { from: model, type: prose } }
  execute-decision:
    model: coder
    prompt: prompts/execute-decision.md
    tools: [tracker.close_issue, tracker.update_issue, tracker.add_comment]
    loop: { maxSteps: 4 }
    mutations: { idempotency: intent-log, onUncertain: refetch-state }  # §6
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
      allowNoop: true
```

Task loop semantics (`completion: model-prose`, the default): render prompt → generate → if output contains a tool-call
fence → gateway executes → append result → repeat → else the text is the node's prose output
→ verify contract → transition. Loop exit without prose after `maxSteps` =
`verification-failed`.

Tasks may opt into `completion: tool-result` to finish an attempt after one
successful tool call, without a follow-up model generation. This mode requires a
nonempty tool allowlist, `loop.maxSteps: 1` (or an omitted loop),
`outputContract.allowNoop: false` (the default), and no model-derived outputs in
the contract or plan-node bindings. Invalid configuration fails before generation.
The single generation must contain exactly one parsed tool-call fence; zero or
multiple calls fail before any mutation intent or tool dispatch. The normal
allowlist, argument/schema validation, mutation journal, gateway error handling,
artifact accounting and output/completeness verification still apply. A successful
gateway return is necessary but does not bypass artifact verification. Authored
`onError` handling, including retries, remains unchanged: this is one call per
attempt, not an at-most-once guarantee across retries or recovery.

Single-call tasks may bind arguments to typed values already in their frame:

```yaml
toolArguments:
  e2llm.read_page:
    fromVars:
      tabId: open_page.outputs.opened.tabId
```

This optional task field requires `completion: tool-result` and names only
granted tools. `fromVars` uses dotted variable paths, without expressions or
filters. All sources resolve before generation; missing values fail the task.
After parsing and checking the tool grant, the executor supplies an omitted bound
argument, accepts an identical model-supplied value, and rejects a conflicting
value before schema validation, mutation journaling or dispatch. JSON types are
preserved. The model can therefore request an operation without choosing an ID
already established by an earlier tool result. Setup's separate `fromInput`
constraints still apply to the effective arguments.

## 5.1 Tool-call fence (the ONLY structured model output)

```
<tool_call>{"server":"tracker","tool":"get_issue","args":{"id":123}}</tool_call>
```

- GBNF grammar constrains fence content to a JSON object with `server`, `tool`, `args`
  (object). Arg-level correctness is validated post-hoc against the tool's input schema;
  invalid args = `tool-call-parse` error (counts against retry/limits).
- Everything outside fences is prose. In the default `model-prose` mode, multiple
  fences per generation are allowed and executed in order; a generation with no
  fence terminates the loop with prose output. `tool-result` mode instead requires
  exactly one fence and does not bind surrounding prose as output.

After the exact task grant check, `ToolGateway::validate_call` checks arguments
before the executor records any mutation intent. `McpGateway::call` also checks
direct callers, including cache and file-served paths. Each server's discovered
tool descriptors and compiled validators are frozen together for the gateway's
captured credential context. Unknown tools and malformed/incomplete catalogs
fail before dispatch. Manual credentials remain frozen for a gateway; OAuth
tokens may refresh within its fixed grant and endpoint.

The pinned [jsonschema validator](https://docs.rs/jsonschema/0.58.6/jsonschema/)
defaults to JSON Schema 2020-12 and recognizes explicit standard drafts 4, 6, 7,
2019-09 and 2020-12. External HTTP/file schema retrieval is disabled. Formats use
the selected draft's default behavior; ordinary 2020-12 `format` annotations are
not extra assertions. Diagnostics identify bounded argument/schema paths without
including argument values or the library's full error payload.

Discovery is bounded to 16 pages, 1024 tools, 4 MiB and a 30-second deadline checked
around requests and schema processing. Each schema is limited to 64 KiB/depth 32;
arguments to 256 KiB/depth 64. Regex compilation/DFA size is limited to 1 MiB and
backtracking to 10,000 steps. These limits do not provide a hard CPU timeout for
arbitrary schema evaluation. Paginated calls treat the caller's page selector as
a positive integer, remove it from the first server request, and validate every
actual request including server-supplied continuation cursors before sending it.

## 6. MCP smart proxy — policy language (normative)

Per-tool policy, keyed `<server>.<tool>`:

```yaml
policies:
  tracker.list_issues:
    class: read
    cache: { key: [query], ttlS: 900 }       # normalized-args hash (sorted keys, defaults applied)
    paginate: { param: cursor, deliver: first_page, backgroundFetch: all }
    shape: { strip: [body], toFile: true }   # stripped fields → artifact; model gets {path, bytes, preview}
  tracker.get_issue:
    class: read
    serveFrom: { fetchedBy: tracker.list_issues, match: id }   # cross-tool serving from file
    onMiss: fetch_live                       # fetch_live | fail
  tracker.close_issue:
    class: mutation
    passthrough: true                        # never cached, never shaped, always logged
    timeoutS: 10
    retry: { attempts: 2, backoffS: 3 }      # transport-level only; never re-executes business logic
```

Rules:

- **One choke point.** Models never talk to MCP directly. All calls go through the gateway
  which applies policy.
- **Cache files**: `cache/mcp/<server>/<tool>/<argsHash>.jsonl` + `.meta.json`
  `{ fetchedAt, ttlS, complete, pages, totalItems?, sourceArgs }`. Atomic writes
  (tmp + rename). Incomplete fetches are marked `complete: false`; readers must treat them
  as `onMiss`.
- **Pagination**: first call returns page 1 synchronously to the model; background task
  fetches remaining pages to the cache file; meta flips to `complete: true` at the end.
  Subsequent paginated calls for the same normalized args slice the file.
- **serveFrom**: resolve `match` field value from args, look up item in the source tool's
  cache file by that key; return it as if the tool answered. `onMiss` governs absence.
- **Mutation journaling** (crash safety): log `tool_call_intent` (tool, args, args hash)
  to the event log BEFORE executing; log `tool_call_result` after. On resume, an intent
  without a result triggers the task's `onUncertain` policy:
  `refetch-state` (query current server state; skip write if already applied) | `fail-run`.
- Every proxy decision (cache hit/miss, reshape, page fetch, rewrite) is an event-log line.

## 7. Event log (normative format)

`runs/<uuid>/events.jsonl`, one JSON object per line, fields:

```json
{ "ts": "...", "runId": "...", "seq": 118, "type": "...", "...": "type-specific" }
```

Event types (closed set): `run_started`, `plan_loaded`, `node_entered`, `node_exited`,
`model_call` (slot, tokens, ms), `tool_call_intent`, `tool_call_result` (or `cache_hit`,
`served_from_file`), `artifact_written` (path, bytes, sha256), `fetch_progress`,
`transition` (from, to, key), `budget_exhausted`, `paused`, `resumed`, `error` (class,
detail), `run_ended` (outcome). `seq` is monotonic per run. Tech debt: hash-chaining
(see SECURITY doc).

## 8. Storage layout (Android app filesDir)

```
trollm/
  flows/<sha256>.flow.yaml  (+ .sig — tech debt)
  tasks/<sha256>.tasks.yaml (+ .sig)
  models/coder.gguf .  reasoner.gguf  (+ .sha256 manifests)
  runs/<uuid>/{events.jsonl, state.snapshot.json, artifacts/}
  cache/mcp/...
```

Flow/task distribution: downloaded bundle = {flow, registry, prompt files, manifest with
sha256s}; verified before install; cached for offline. Plan pins `sourceBaseline.sha256`;
app verifies registry+weights hashes match manifest.

## 9. Crate contracts (pin these signatures)

### trollm-models

```rust
pub enum FinishReason { Stop, Length, GrammarReject }
pub struct GenerateRequest { pub prompt: String, pub grammar_gbnf: Option<String>,
                             pub max_tokens: u32, pub stop: Vec<String> }
pub struct GenerateResponse { pub text: String, pub tokens_in: u32,
                              pub tokens_out: u32, pub finish: FinishReason }
pub trait ModelSlot: Send {
    fn generate(&mut self, req: GenerateRequest) -> anyhow::Result<GenerateResponse>;
}
pub struct MockModel /* scripted responses for tests */;
// feature "llama": LlamaCppSlot::new(path, ctx) — llama-cpp-2 crate; GBNF passed per call.
```

### trollm-mcp

```rust
pub struct ToolDescriptor { pub server: String, pub name: String,
                            pub input_schema: serde_json::Value }
pub struct ToolCall { pub server: String, pub tool: String, pub args: serde_json::Value }
pub struct ToolResult { pub content_for_model: serde_json::Value,
                        pub artifacts: Vec<PathBuf>, pub provenance: Provenance }
pub enum Provenance { Live, Cache, ServedFromFile }
pub trait ToolGateway: Send {
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>>;
    fn validate_call(&self, call: &ToolCall) -> anyhow::Result<()>; // before mutation intent
    fn call(&mut self, call: &ToolCall) -> anyhow::Result<ToolResult>; // applies policies
}
// McpGateway: streamable-HTTP client + PolicyEngine; emits events via a sink callback
// into the core's event log. MCP transport: use `rmcp` crate if it cleanly supports
// streamable HTTP client; otherwise hand-rolled JSON-RPC over reqwest. (May decide.)
```

### trollm-core

```rust
pub struct ExecutionPlan { /* serde model of §3 */ }
pub struct TaskRegistry { /* serde model of §5 */ }
pub struct Executor<M: ModelSlot, G: ToolGateway> { /* ... */ }
impl<M: ModelSlot, G: ToolGateway> Executor<M, G> {
    pub fn new(plan: ExecutionPlan, registry: TaskRegistry, user_input: String,
               models: SlotMap<M>, gateway: G, run_dir: RunDir) -> anyhow::Result<Self>;
    pub fn run_until_blocked(&mut self) -> anyhow::Result<RunStatus>; // Paused | Succeeded | Failed
    pub fn resume(&mut self) -> anyhow::Result<RunStatus>;
    pub fn cancel(&mut self);
    pub fn snapshot(&self) -> anyhow::Result<()>;   // state.snapshot.json (also written each transition)
}
pub enum RunStatus { Running, Paused { reason: String }, Succeeded, Failed { reason: String } }
```

`Executor` is generic over traits; all unit/integration tests use `MockModel` and a mock
gateway. No network, no real models in core tests.

### trollm-uniffi

Thin UniFFI wrapper exposing: `list_flows()`, `install_flow_bundle(bytes)`,
`start_run(flow_sha, user_input) -> run_id`, `poll_status(run_id)`, `pause/resume/cancel`,
`tail_events(run_id, from_seq)`. No logic beyond marshalling.

## 10. Android app

- Kotlin + Jetpack Compose. Min SDK 28 (subject to llama.cpp/UniFFI constraints — may
  adjust, non-consequential).
- UniFFI bindings from `trollm-uniffi` (generated, not handwritten).
- Runs execute in a **foreground service** with partial wake lock; UI observes by tailing
  the event log (no separate progress API).
- Secrets (MCP bearer tokens) in Android Keystore-backed EncryptedSharedPreferences;
  TLS-only egress to plan-declared MCP server hosts (network security config allowlist).
- Models + flow bundles sideloaded/downloaded over the enterprise channel; sha256 verified
  at install (airgap path: USB/ADB bundle transfer).
- App never parses prose; renders run progress from event-log entries only.

## 11. Testing strategy

- **core**: conformance tests executing `flows/examples/*` end-to-end with MockModel +
  scripted mock gateway; assert event-log sequences, transitions, budget enforcement,
  pause/resume mid-foreach, crash-recovery (kill between intent and result → onUncertain),
  onError retry paths.
- **mcp**: policy engine tests against a local mock MCP HTTP server (pagination, cache TTL,
  serveFrom hit/miss, shape-to-file, mutation passthrough + journaling).
- **models**: MockModel contract tests; llama slot smoke test behind feature flag, skipped
  without weights.
- **android**: build-only verification deferred (no SDK on dev machine); unit-testable logic
  must live in the Rust core, not in Kotlin.

## 12. Milestones

- **M1 (agents now)**: crates core + models(mock) + mcp(mock-server tests) + example flows +
  JSON schema. `cargo test --workspace` green.
- **M2**: llama.cpp adapter on a Linux dev box with real weights; GBNF fence tuning;
  measure tok/s + tool-call parse reliability (go/no-go for coder model choice).
- **M3**: UniFFI + Android app skeleton; sideload flow bundle; first on-device run.
- **M4**: security tech-debt burn-down (signatures, hash chaining, mTLS).

## 13. Decision points (may decide vs must ask)

Documentation rule for all contributors and agents: docs, READMEs, and code comments
contain only project-specific technical content — mechanisms, decisions, constraints,
verification status. No generic explainers (what MCP/enterprise security/auditability
"is"), no process or team-culture prose. If a concept needs explaining, link out; don't
paraphrase.

Agents MAY decide: internal module organization within a crate, serde vs manual YAML,
`rmcp` vs hand-rolled JSON-RPC (per §9), tokio vs std-threads for background fetch,
exact GBNF text, test naming.

Agents MUST STOP AND REPORT (not assume): any change to §3–§9 formats or signatures;
adding/removing node kinds, transition keys, event types, error classes, policy keys;
any deviation from the design invariants in PREMISE.md; any new third-party dependency
beyond: serde, serde_yaml/serde_json, anyhow, thiserror, reqwest(or rmcp's transport),
tokio(if needed), uuid, sha2, llama-cpp-2(feature-gated), uniffi(in uniffi crate only),
parking_lot/once_cell-style utils.

## 14. Ratified implementation decisions (post-M1)

Ambiguities reported by M1 implementation agents, resolved by the design owner. These
refine §3–§9 without changing them:

1. **Counter scope**: budgets are enforced per frame (each flowRef frame enforces its own
   plan's limits); `RunState.counters` is the run-wide aggregate mirror. Rationale: the
   example flows only work per-frame (`return-to-list` gets maxSteps:2 per invocation).
2. **`maxItems`**: per-dispatch-session window; resets on resume. The foreach cursor is
   absolute and persists. Otherwise `maxItems → paused` could never resume.
3. **Artifact logical names**: `fromArtifact: tracker.list_issues.latest` maps to gateway
   file `tracker.list_issues.latest.jsonl` + `.meta.json`; logical name = filename stem;
   core falls back to scanning the run artifact dir by name.
4. **Error-class edge mappings** (closed enum unchanged): tool outside task allowlist →
   `tool-call-parse`; template render failure (unbound `{{ var }}`) → `verification-failed`;
   log/snapshot I/O failure → `artifact-io`.
5. **Tool-call grammar root**: `ws (fence+ | prose)` where prose may not contain the
   fence-open token; grammar is applied to every coder-slot generation. (M1 shipped
   fences-only, which made prose loop-termination impossible on constrained steps.)
6. **MCP pagination response shape**: items = first array-of-objects field in the payload
   (or top-level array); cursors are 1-based integer page numbers for file slicing, with an
   internal `<hash>.pages.json` index. Opaque string cursors from real servers = **M2
   decision point** (current fallback: serve page 1 from cache).
7. **Mutation journaling wiring**: gateway emits `MutationPassthrough` BEFORE executing
   (core journals as `tool_call_intent`), `LiveCall` after; on resume, dangling intents are
   closed as skipped per `onUncertain`; the real state-refetch lands with real servers in M2.

## 15. Wrapper-integration decisions (post-M3) and core gaps

Decisions ratified from the UniFFI/Android implementation; the two "core gap" items are
accepted tech debt with specified fixes — they are the first work items of the next
iteration.

1. **Bundle format**: uncompressed ustar (hand-rolled reader, zero new deps — §13 admits no
   `zip` crate). Integrity comes from manifest sha256s (fail-closed). Switching to ZIP is an
   owner decision (adds `zip` to the allowlist).
2. **UniFFI mechanics**: proc-macro mode (`uniffi = "0.28"`), bindgen via a feature-gated
   shim binary inside the wrapper crate (pins bindgen to the lockfile's UniFFI).
3. **CORE GAP A — proxy events into the main event log.** §9 intended the gateway sink to
   write into the run's `events.jsonl`, but `Executor` owns `EventLog` seq internally; a
   second writer breaks §7's monotonic seq. Stopgap (accepted): gateway events go to
   `runs/<uuid>/proxy.events.jsonl` (same envelope, own seq); `tail_events` serves the main
   log only, so `cache_hit`/`fetch_progress` don't reach the UI yet. **Specified fix**:
   core exposes a shared sequenced sink (`Arc<Mutex<EventLog>>`) that the gateway writes
   through; sidecar is retired.
4. **CORE GAP B — user-initiated pause/cancel of a running run.** `run_until_blocked` has
   no step API, so a foreign thread cannot inject `cancel()` mid-dispatch. Stopgap
   (accepted): cooperative hold at the next model/tool boundary for pause; cancel trips a
   marker error ending the run as failed/cancelled. **Specified fix**: core gains a
   `cancel_flag: Arc<AtomicBool>` checked between nodes (and between coder-loop steps),
   producing a real `paused`/`run_ended{cancelled}` event.
5. **`user_input` persistence**: not in `RunState`; wrapper stores
   `runs/<uuid>/user_input.txt` sidecar so cold resume re-renders prompts faithfully.
   Additive; a later core version may absorb it into RunState.
6. **`sourceBaseline.sha256` semantics** ("authored against" what, exactly) remain undefined
   by design for now; wrapper verifies registry+weights against the *bundle manifest* (§8)
   and deliberately does not enforce plan↔registry equality.
7. **Final report access**: §9 has no result accessor; on `Succeeded`, `poll_status` derives
   `final_report` from the run's own records (terminal node's bound prose outputs). A core
   `run_result()` accessor is the cleaner long-term API — same iteration as gaps A/B.

## 16. Constraints after baseline-review remediation

The constraints below summarize the baseline review remediation.
These implementation choices keep the current prototype bounded:

1. **Listing publication:** a paginated call collects the complete dataset before
   returning the requested page and publishing `<server>.<tool>.latest.jsonl` plus
   its completeness sidecar. Existing `deliver: first_page` / `backgroundFetch: all`
   declarations remain accepted, but no detached fetch runs. Limits are 256 pages,
   100,000 items, 32 MiB of serialized items and 120 seconds overall; response bodies
   received from MCP are capped at 32 MiB, and individual requests default to 30 seconds.
   Cooperative pause/cancel takes effect at the next boundary after that bounded call.
2. **Cache and credential ownership:** each gateway has a private cache namespace;
   sequential calls in a run reuse it, while cross-run reuse is disabled. Manual tokens
   are captured once after endpoint validation. OAuth callbacks stay bound to one
   saved local grant and endpoint while refreshing tokens. Invalidated grants fail
   before network calls or cached results; a new account requires a new run.
   Old shared entries are ignored; cache retention remains an application concern.
3. **MCP failures and egress:** production endpoints require HTTPS and redirects are
   rejected. HTTP loopback is an explicit fixture API. Tool `isError` envelopes fail
   the call without entering success caches. Mutations receive one transport attempt;
   configured retry counts do not permit replay after ambiguous response loss.
4. **Recovery:** child return values/outcomes persist with the parent identity before
   the child frame disappears. A completed journaled mutation after the last committed
   snapshot causes recovery to fail before generation. This avoids unsafe replay;
   it does not provide automatic reconciliation or exactly-once external effects.
   Legacy snapshots without an event watermark are handled conservatively.
5. **Bundle integrity:** install, identical reinstall and startup check prompt and
   subflow references against the verified manifest/root. Reinstall repairs missing
   or corrupt canonical pins. Model checksum verification is exercised independently
   of the native inference feature using a standard digest vector.
6. **Controls and Android:** cooperative holds share one wait-predicate mutex with
   resume/cancel. Only an acknowledged authored pause accepts an authored resume
   command. Android persists/discovers run UUIDs and the foreground service retains
   ownership until all attached runs complete. Native library packaging and device
   lifecycle behavior require the Android CI/device checks described in the app README.
7. **Tool input validation:** the fix for
   INPUT-003 (tool argument schema validation) adds schema
   validation before mutation journaling and on direct gateway calls, as detailed
   in §5.1. This checks arguments against the advertised contract; it does not
   establish business authorization or reconcile uncertain external effects.

## 17. Setup and result handoff additions

A root flow may carry an optional `setup: {path: setup.json, sha256: <digest>}`.
The JSON descriptor uses `format: trollm.flow-setup.v1`; its complete shape is
in [the setup schema](../schemas/flow-setup.v1.schema.json). It contains a title,
optional description/input labels, model sources and settings keyed by slot,
and connection defaults keyed by declared server name. Credentials stay outside
the bundle. Models cover the whole flow tree; default contexts and server URLs
must agree with its declarations. Only the root declares setup. Old bundles
without it retain their existing behavior.

The descriptor must be a verified manifest member and match the hash in the
root flow. Changing its contents requires changing the root pin, giving the flow
a new content address. Install, startup and `flow_setup` use this same boundary.
This adds setup metadata to the existing graph, without introducing new nodes.

`input.fields` optionally declares one to eight text or HTTP(S) URL fields. Each
has a unique ASCII identifier, display label, optional placeholder, required flag
and maximum length (default 4096, at most 16384 Unicode code points). Android
renders these fields and submits an object of strings through the existing
string input API. The bridge rejects unknown keys, trims values, fills omitted
optional fields with empty strings and enforces a 64 KiB serialized UTF-8 limit.
URL fields require a host and forbid embedded credentials. The canonical JSON
is persisted as the run input; cold resume validates it against the pinned setup.
Without fields, legacy input remains a verbatim string.

Optional `input.toolArguments` adds immutable restrictions for at most 16 exact
`server.tool` names: `fromInput` binds arguments to field values, `fixed` binds
JSON constants, and `forbid`/`require` restrict argument presence. Conflicts and
unknown input keys fail setup validation. The bridge derives bounded offline
schemas; the gateway checks them before mutation intent logging and dispatch,
alongside the server's tool schemas. It rejects mismatches instead of rewriting
model-generated arguments. The browser starter uses these checks to bind every
call to the selected browser and the opening action to the requested URL, and
forbids `sessionId` because the server can use it to override browser selection.

Android's preparation screen uses DownloadManager for one transfer at a time.
The app verifies byte count and SHA-256 before publishing a hash-named GGUF in
app-specific external storage. Existing private-slot files can be reused after
matching the declared identity. No model-sized copy is needed at publication.
Interrupted downloads remain system-owned; the app reconciles saved download
IDs and removes identifiable orphaned partial transfers on return.

`start_run_configured` accepts app-resolved JSON with `version: 1`, a `models`
map (`path`, `sha256`, `ctx`, `temperature`, `topP`) and a `connections` map
(`url`). The bridge persists normalized effective settings in the run's
`configuration.json`; cold resume reloads them. Native startup verifies weight
hashes and applies the recorded sampling settings. New native runs using legacy
slot files also capture their settings; old snapshots remain readable.
Pre-executor failures persist in `worker-failure.json`, and successful executor
reconstruction clears an obsolete failure. The UI defaults to one live run;
the core still supports explicit concurrent callers.

An ordinary read or mutation policy can set `resultArtifact: page` to publish
the parsed tool JSON as `page.json` before any shaping. Names match
`[A-Za-z0-9][A-Za-z0-9_-]{0,63}`; paginated policies cannot use this
field. Live, cache and `serveFrom` paths share publication behavior. A successful
replacement is atomic and gains a complete sidecar; a local publication failure
does not mark new data complete. Existing `fromArtifact: page, format: json`
bindings let the next task consume the actual result independently of model
prose. Task errors still follow their authored error transitions.

Ordinary read and mutation policies may also declare
`resultValidation: {maxBytes: 16384, schema: {...}}`. Both fields are required;
`maxBytes` accepts 1–262144 serialized
JSON bytes. The gateway compiles the schema once with the existing bounded,
offline validator and checks the original payload before shaping, caching or
marking a named artifact complete. Live, cache-hit and `serveFrom` results use
the same check. Rejected replacements invalidate the old artifact's completeness
marker; a read retry can fetch a corrected upstream response. Paginated policies
cannot use this option. Each mutation gateway call dispatches once: validation,
publication or shaping failure after its remote effect propagates without a
gateway retry. Existing journal/recovery and authored error policies still apply.
Errors identify the failing schema path without
including result values. A browser flow can therefore require nonempty content,
positive content-node counts and `stats.truncated: false`. This depends on the
server reporting truncation accurately. The byte cap is not a model-token
guarantee; the native model separately checks its rendered prompt against its
configured context before prefill.

Browser starter v4 saves the validated `act(open)` result as `opened.json`, then
binds its positive integer `tabId` directly into `read_page`. It does not list
other tabs or ask a model to match URLs. Reads stay in the just-opened tab,
including ordinary redirects; the final answer uses the actual returned source
URL. The complete page artifact, still capped at 8192 serialized bytes, goes to
the tool-free reasoner. This runtime contract requires a new APK; importing the
v4 bundle into an older runtime does not establish support for its bindings.

The Android OAuth client uses AppAuth with browser PKCE, encrypted state and
endpoint-bound local grant IDs. Refresh updates use the same grant identity so
logout or replacement login invalidates late callbacks. The callback is
`com.trollm.app://oauth/callback`. OAuth metadata is mapped explicitly; the E2LLM
client requests MCP access rather than an unused identity token. Its optional
operator PEM augments platform roots while retaining normal hostname checks.
The app now supplies the selected authentication mode and grant-aware token
callback to Rust. Each run records only modes, endpoints and local grant IDs in
`connection-auth.json`; it never stores access or refresh tokens there. Cold
resume reuses those identities. Older runs without this file retain manual
authentication but cannot silently adopt a newly signed-in OAuth account.
Logout, failed sign-in and pending sign-in do not reactivate a retained manual
token; explicitly saving a manual token selects that mode and invalidates the
old grant. HTTP failures do not replay mutations. Real sign-in, refresh and
device lifecycle acceptance remain to be exercised on the compiled APK.
