# trollm — Android Architecture (v1 target)

The approved [coder-directed flow extension](ADAPTIVE-FLOWS.md) adds explicit
coder control tools, authored routes, node instructions, recovery context and
per-task generation limits. It supersedes the static-routing restriction below
for opted-in tasks; legacy task/foreach/end behavior remains supported. This
extension is implemented in source and does not add a model-free tool node.

The later [direct tool execution contract](DIRECT-TOOL-EXECUTION.md) adds an
explicit `execution: tool` registry mode within the existing `task` node kind.
It invokes one fully bound ordinary tool through the same checked dispatch path,
without a model or prompt. Default model tasks retain their existing behavior.
Selected-guard calls are excluded from direct execution. This source capability
is newer than the published 1.0.1 APK.

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
  promptTemplate: "..."                     # reserved metadata; not rendered

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

`input.promptTemplate` is reserved metadata and is not rendered by this runtime.
The task-registry `prompt` file supplies the model prompt, including supported
bindings such as `{{ user_input }}`. Changing the reserved field alone has no
effect on generation.

New bundle imports require a present `sourceBaseline.sha256` to be 64 lowercase
hexadecimal characters, in both plans (including subflows) and the registry.
Installed v1 bundles with older string identifiers remain readable, and an exact
verified reinstall can repair their canonical pins without changing their bytes.
Provenance remains optional descriptive metadata, not a trust anchor. Closed
plan, node, registry and policy objects reject unknown keys; intentionally named
maps (nodes, slots, outputs, tool arguments and result schemas) remain extensible.
Node IDs contain 1–128 ASCII letters, digits, underscores or hyphens. Validation
enforces this in root and child plans before execution because IDs also appear in
snapshot filenames. Artifact writers independently reject path-like filenames.
Previously accepted IDs outside this grammar must be renamed, along with their
references, before importing or running the flow on 1.0.1 or later.

### 4.1 Node kinds — closed set

`task` | `foreach` | `end`. Nothing else in v1. A reasoner consult is a `task` whose
taskRef binds the reasoner slot and grants no tools.
The literal `reasoner` slot is enforced as tool-free during registry and executor
validation. Other slot names do not acquire an inferred semantic role.

### 4.2 Transition keys — closed set

`onVerified`, `onComplete`, `onEmpty`, `onLimit`, `onError`, and the authored
`routes` map for `completion: flow-route` or `tool-or-route` tasks. `flow-route`
uses routes instead of `onVerified`; `tool-or-route` requires both ordinary
success and authored control exits. Values are node ids or inline
`{ status: paused|failed, reason: string }`.
A paused route also supplies `resumeAt`, an existing continuation node persisted
with the paused state. It resumes there without replaying the completed chooser.
Legacy inline pauses outside routes can omit this field and keep their existing
behavior. There are no expressions or conditions on model prose. A coder requests
a declared route through `flow.choose`; the runtime verifies and applies it.
A missing transition key for an outcome that occurred = plan validation error at
load time where statically checkable, else runtime `failed` with reason `unhandled-transition`.

### 4.3 Verification (ledger-side, deterministic)

Per task, the registry declares an output contract. Verification checks **structure only**:
required outputs exist, types match, artifact references exist and (for `fromArtifact`
outputs) the meta sidecar says `complete: true`. Verification NEVER inspects model prose.
A `task` whose contract accepts a no-op must say so (`allowNoop: true`) — triage's `act`
step is the canonical case.

Artifact output bindings in either a task contract or a plan node may add
`view: {fields: {id: true, attributes: {href: true}}, maxBytes: 6144}`.
The nonempty recursive field mask selects exact JSON keys; `true` copies the
selected branch, and an object applies the nested mask. Arrays apply the same
mask to every item, including parsed JSONL records. Missing keys are skipped;
a present scalar where an object mask is required fails verification. Views do
not filter, sort, interpret text, or modify saved artifacts. `maxBytes` is required
(1 through 1,048,576) and bounds the compact UTF-8 JSON representation of the
selected output; overflow fails without truncation. Omit `view` to retain the
existing complete output. Keep separate raw and view bindings when later code
needs the complete data while the next model needs only selected fields.

### 4.4 Executor = stack machine

- `flowRef` pushes a call frame (plan id + sha256, bound inputs, own counter scope).
- `foreach` executes: snapshot `items` once → per item push body frame → after each item,
  push `between` frame → aggregate per §4.6.
- Run state (serializable at every node boundary):

```json
{
  "runId": "uuid", "status": "running|paused|succeeded|failed",
  "stack": [ { "plan": "ticket-triage@sha256:...", "node": "each",
               "vars": { "...": "..." },
               "foreachCursors": { "each": { "index": 42, "snapshotRef": "artifacts/....jsonl" } } } ],
  "foreachCursors": {},
  "counters": { "steps": 118, "reasonerCalls": 2, "tokens": 41000, "startedAt": "..." }
}
```

- Pause/resume: `paused` state is persisted; resume = load state, continue dispatch.
  Crash recovery: identical path (resume from last persisted boundary; at most one node
  is re-executed — see §6 mutation journaling for write safety).
- Counters persist across pause/resume. Per-node budgets (e.g. `maxSteps` on a coder-loop
  task) reset on node entry; run-level budgets never reset.
- Failed and cancelled runs save an optional `terminalReason` alongside the
  existing status. Older snapshots without it remain readable. Cancellation
  records `run_ended` with `outcome: cancelled` and remains `Failed` with reason
  `cancelled` in the public API.

### 4.5 onError

Every `task` may declare `onError: { retry: N, backoffS: S, then: <nodeId|{status...}> }`.
Default if absent: `retry: 0`, then `{ status: failed, reason: <error-class> }`.
Each `backoffS` must be an integer from 0 through 300 seconds inclusive, including
in loaded subflows. Each wait is also capped by the remaining active-frame run
deadline; expiry applies `onLimit` before another attempt. Pause/cancel remains
cooperative, with an explicit user hold checked again when resumed.
Automatic retry is rejected for tasks granted mutation tools and for any task
declaring `mutations.idempotency: intent-log`, including a journaled read-only task.
Read-only tasks without intent logging retain automatic retries. This prevents
whole-task retries from repeating a possibly applied write; explicit authored
graph transitions remain separate flow control.
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
    mutations: { idempotency: intent-log, onUncertain: fail-run }  # §6
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
      allowNoop: true
```

### Task-specific result artifact names

A single-generation `tool-result` task granting exactly one non-paginated remote
read tool may declare `resultArtifact: selected-original`. Its root tool policy
must already declare a result artifact; `serveFrom`, local/control tools and
mutations are excluded. The optional task field changes only this dispatch's
native storage destination. The model does not supply it and wire arguments,
credentials, validation, cache identity, deadlines and classification are unchanged.
Names use existing safe basename bounds and cannot collide with reserved native
artifacts or another tool's destination. Same-tool producers may deliberately
share a name across authored branches.

Core clears old producer outputs and invalidates the target before generation.
Failure to invalidate ends the run, bypassing authored retries/error routes that
could otherwise read a still-complete old file. Normal success requires the named
artifact from the current call. Gateway direct
calls invalidate before transport/cache access and publish through existing
validation, hash and completeness paths. Unsupported adapters fail closed. An
alias published from cache retains Cache provenance; publication is not recapture
or proof of freshness. Original and narrow reads can therefore retain separate
artifacts without copying prose, interpreting DOM, adding a model call or changing
mutation recovery. Omission preserves old task behavior. Runtimes predating this
field reject it; an APK containing the updated Rust code is required. See the
[authoring guide and limits](TASK-RESULT-ARTIFACTS.md) and
[local schema](../schemas/task-result-artifact.v1.schema.json).

Task loop semantics (`completion: model-prose`, the default): render prompt → generate → if output contains a tool-call
fence → gateway executes → append result → repeat → else the text is the node's prose output
→ verify contract → transition. Loop exit without prose after `maxSteps` =
`verification-failed`.

Every `model_call` records `finishReason`: `stop`, `length`, `timeout` or `grammar-reject`.
Length-limited, timed-out or grammar-rejected responses fail as `model-error` before prose
acceptance or tool-call parsing/dispatch, even if their text contains a complete
fence. No automatic continuation is added. Token use is still counted; if that
response also exceeds its frame's token budget, the existing `onLimit` policy
takes precedence and cannot consume the rejected response as a valid result.

Artifact outputs may include an optional inline JSON Schema object:

```yaml
outputContract:
  outputs:
    data:
      fromArtifact: observation
      format: json
      type: object
      schema:
        type: object
        required: [metadata]
        properties:
          metadata:
            type: object
            required: [url]
            properties:
              url: {type: string, minLength: 1}
```

The registry validates authored schemas when loaded, using the existing offline
JSON Schema engine and its 64 KiB/schema-depth-32/regex bounds. External references
are not fetched. Schemas are JSON objects, not null or boolean values, and are
accepted on `fromArtifact` and `from: model` outputs. There is no interpolation or expression
evaluation inside a schema.

Verification checks the complete original parsed JSON value (or the complete array
parsed from JSONL) before a `view`, output binding or successful transition. Its
byte allowance follows the active plan's artifact budget, with the existing
schema-engine depth-64 limit; the 256 KiB tool-argument limit does not apply.
Schema mismatches report a redacted `verification-failed` error and follow the
node's existing `onError`. No output from the failed node is bound; original
artifacts remain available as evidence. Existing completeness and coarse output
type checks still apply.

If a node overrides an output's artifact binding, its schema checks the effective
original artifact before that node's view. A schema-bearing artifact output cannot
be overridden by a model output. Omitting `schema` preserves earlier behavior.

Model outputs can use the same optional schema object:

```yaml
outputContract:
  outputs:
    query: {from: model, type: string, schema: {type: string, minLength: 1}}
```

Validation checks the exact trimmed model string that will be published. It does
not parse prose as JSON, extract fields, interpret query meaning or modify the
string beyond the existing outer trim. Empty or whitespace-only responses fail
the example contract. A same-source node-level model override still validates
that string; an artifact override of a schema-bearing model output is rejected
when the plan and registry are validated.

The check runs while the original generation request and response are available,
before model output binding or transition. A mismatch returns a redacted
`verification-failed` error through existing `onError`. The existing private
diagnostic writer may retain the original untrimmed failed response, prompt and
decoder constraints, subject to its existing bounds and artifact allowance.
Failure to save diagnostics cannot replace the contract failure. Successful
model schema validation needs no artifact allocation and adds no value-byte cap
to the existing generation limits; schemas still use the bounded offline engine.
Model outputs without a schema keep their previous behavior, including accepting
an empty string.

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

Tasks may instead use `completion: flow-route`, with the literal `coder` slot,
an explicit `flow.choose` grant and node-level authored routes. Ordinary tools
may run in its bounded loop, but only a single `flow.choose` call in its own
generation can complete the task. Mixed control/action generations reject before
dispatch. Model-derived outputs are forbidden; artifact verification still runs.
The control call never goes to the MCP gateway. See the
[adaptive-flow contract](ADAPTIVE-FLOWS.md) for the complete interface, prompt
supplements, runtime context and persisted recovery information.

Tasks may use `completion: tool-or-route` for one coder generation that either
invokes an ordinary tool or chooses an authored route. The node declares both
`onVerified` and `routes`, the task grants `flow.choose` and an ordinary tool,
and `loop.maxSteps` is one. Ordinary tool success must publish the current-call
artifacts and pass output verification before `onVerified`. An authored route
publishes no ordinary success outputs; previous output aliases for this node
are cleared before a new attempt and stay absent after decline and Resume.
Bound ordinary arguments remain supported; bound `flow.choose` arguments,
model-derived outputs, prose completion and mixed or multiple calls are forbidden.
A failed tool call follows `onError`, not a semantic decline. Mutation journaling,
unknown-outcome handling and retry restrictions still apply. See the
[complete mode contract](ADAPTIVE-FLOWS.md#one-tool-result-or-one-authored-route).

Optional task `maxOutputTokens` (1 through 2048) and `generationTimeoutS`
(1 through 600) bound each generation. Omission preserves the original token
ceiling and supplies no additional deadline. Native deadlines are cooperative
between prefill batches and tokens. They do not preempt one decode operation
or weaken length/grammar-rejection failure handling.
The generation deadline is also capped by the earliest remaining deadline among
active frames, including the root. `maxOutputTokens` remains an upper bound:
native generation can use smaller remaining context headroom. Native responses
measure context size, remaining tokens and the effective output ceiling; private
failure evidence distinguishes the requested ceiling from context exhaustion.
Native generation timeouts preserve available partial response text and counts.
A response returning after the run deadline is saved as private failure evidence
before the runtime applies `onLimit`; its prose or calls cannot complete a task.
Android cooperative holds, weight verification and model loading consume the
same allowance. An expired allowance prevents starting inference after loading;
one synchronous native load or decode still cannot be preempted.

Single-call tasks may supply arguments from frame values, structured input or
literal constants:

```yaml
toolArguments:
  e2llm.act:
    fromInput:
      browser: browser
      target: url
    modelFields: []
    fixed:
      action: open
  e2llm.read_page:
    fromInput:
      browser: browser
    fromVars:
      tabId: open_page.outputs.opened.tabId
```

This optional task field requires `completion: tool-result` or `tool-or-route` and names only
granted tools. `fromVars` uses dotted frame-variable paths, without expressions
or filters. `fromInput` reads exact top-level keys from a JSON-object user input;
it does not interpret dotted paths. `fixed` contains literal typed JSON values,
without interpolation. A rule must contain at least one binding or `modelFields`, and an argument
cannot appear in more than one source map. All sources resolve before generation;
missing values or non-object input for `fromInput` fail the task. Tasks without
`fromInput` retain support for ordinary text input.
At an array, a dotted path segment containing only unsigned decimal digits selects
that index, for example `saved.outputs.tabs.tabs.0.id`. Object keys, including
numeric keys, retain literal lookup. Missing, out-of-range and overflowing indices
remain unbound; selected values retain their original JSON types.
After parsing and checking the tool grant, the executor supplies an omitted bound
argument, accepts an identical model-supplied value, and rejects a conflicting
value before schema validation, mutation journaling or dispatch. JSON types are
preserved, and the journal records the final effective arguments and their hash.
The model can request an operation with `args:{}` without copying user-provided
URLs or choosing IDs established by earlier tool results. Setup's separate
`fromInput` constraints still apply to the effective arguments; their validation
semantics are unchanged. Browser starter v5 opts into the new task-level input
and constant bindings. Earlier runtimes reject those new fields rather than
silently ignoring them; existing registries using only `fromVars` remain valid.

An optional per-tool `modelFields` list closes the model-owned top-level argument
names. `modelFields: []` requires `args:{}`; an explicit list permits only those
names, with required fields taken from known tool/policy requirements rather than
making every listed field mandatory. Names must be unique identifiers and cannot
overlap runtime-bound arguments. Missing required unbound fields fail before
generation. Closed argument generation remains active when discovery is unavailable;
unknown names, reintroduced bound names and duplicate argument names in model
output fail before bindings or dispatch. The original tool schema still validates
the effective arguments after runtime bindings preserve their typed values.
Omitting `modelFields` preserves existing open-schema and matching-bound-value
behavior. Explicit null is rejected. This field restricts names, not nested JSON values or site semantics.

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

The gateway also supplies the reserved `local.calculate` built-in described in
[Built-in tools](BUILTIN-TOOLS.md). It uses the same coder grants, tool-call
validation, result policies and artifact bindings, with no remote server or
credentials. The reasoner remains tool-free. A flow using this capability needs
an APK built from the implementation; a bundle update alone cannot add it.

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
    retry: { attempts: 1, backoffS: 0 }      # optional; only these defaults are supported
```

Rules:

- **One choke point.** Models never talk to MCP directly. All calls go through the gateway
  which applies policy.
- **Cache files**: `cache/mcp/<server>/<tool>/<argsHash>.jsonl` + `.meta.json`
  `{ fetchedAt, ttlS, complete, pages, totalItems?, sourceArgs }`. Atomic writes
  (tmp + rename). Incomplete fetches are marked `complete: false`; readers must treat them
  as `onMiss`.
- **Pagination**: a cache miss synchronously fetches the bounded result set before
  returning the requested page. `backgroundFetch: all` does not spawn detached work.
  Pagination may declare `responseCursor: /pagination/cursor` and
  `responseHasMore: /pagination/hasMore` as RFC 6901 JSON pointers. Without a
  response cursor pointer, the response's top-level request `param` key supplies
  continuation as before. An explicit `hasMore: false` ends fetching, even with
  a stale cursor; `true` requires a non-null cursor. Missing metadata on a bare
  array ends fetching. Response cursor values pass unchanged to the same request
  argument; cache argument identity and all existing fetch bounds are preserved.
  Meta becomes `complete: true` after completion; subsequent calls slice the file.
- **Transport retry**: omitted/empty `retry` and `attempts: 1, backoffS: 0` are
  accepted. Other values are rejected as unsupported before dispatch. Mutations
  remain single-attempt; this does not change task-level `onError` restrictions.
- **serveFrom**: resolve `match` field value from args, look up item in the source tool's
  cache file by that key; return it as if the tool answered. `onMiss` governs absence.
- **Mutation journaling** (process-crash safety): tasks granted a tool classified
  as a mutation by the root policy or effective gateway must declare intent logging.
  Validation includes nested subflows; the dispatcher checks classification again.
  Core logs `tool_call_intent` before executing and `tool_call_result` after.
  An unresolved intent stops recovery with an explicit uncertain-write failure.
  Legacy `refetch-state` still parses but also fails closed: no reconciliation query
  is implemented, and no synthetic result or automatic redispatch is allowed.
  Completed journaled writes beyond the snapshot watermark also block replay.
  These checks use authored policy classifications; they cannot infer a remote
  tool's actual side effects or establish power-loss durability.
- **Filesystem paths**: cache/worklist server and tool components, and shaped
  JSON field components, use deterministic safe names. Ordinary short ASCII
  alphanumeric/underscore/hyphen names stay readable; other names use a reserved
  prefix plus SHA256. Cache keys must be lowercase SHA256 hex. Remote names stay
  unchanged on the wire, and unsafe old paths are never used as a fallback.
- Proxy decisions are written to the separate `proxy.events.jsonl` audit log;
  they are not currently merged into the main log or its UI event stream.

## 7. Event log (normative format)

`runs/<uuid>/events.jsonl`, one JSON object per line, fields:

```json
{ "ts": 1700000000000, "runId": "...", "seq": 118, "type": "...", "...": "type-specific" }
```

Event types (closed set): `run_started`, `plan_loaded`, `node_entered`, `node_exited`,
`model_call` (slot, tokens, ms, finishReason), `direct_tool_step` (task, tool, charged attempt),
`tool_call_intent`, `tool_call_result` (or `cache_hit`,
`served_from_file`), `artifact_written` (path, bytes, sha256), `fetch_progress`,
`transition` (from, to, key), `budget_exhausted`, `paused`, `resumed`, `error` (class,
detail), `run_ended` (outcome). `seq` is monotonic per run. Tech debt: hash-chaining
(see SECURITY doc).
The core `cache_hit`, `served_from_file` and `fetch_progress` variants are reserved;
the current gateway emits its own sidecar records instead.
`direct_tool_step` is emitted before a direct attempt resolves bindings. It
consumes one frame/run `maxSteps` unit even if validation later fails. It is
not a model call or proof that a tool was dispatched; direct attempts consume
no model tokens or reasoner calls. Existing model responses keep their existing
step accounting. The attempt context joins direct steps to actual tool outcomes.

Open/recovery and reads reject malformed nonblank records, invalid envelopes,
mixed run identities and duplicate/decreasing sequence numbers. Positive
increasing sequence gaps and blank lines are allowed. Corrupt bytes are retained;
errors identify the file and line without reproducing tool arguments. A complete
valid last record lacking a newline gets a delimiter before the next append;
torn JSON never gets silently skipped or repaired. Payload fields cannot replace
the envelope. Flush-before-dispatch remains a process-crash ordering measure,
not an fsync or power-loss durability guarantee.
Older logs that violate these checks can no longer resume even if an earlier
reader accepted them. Their bytes are preserved for deliberate recovery; no
automatic repair or mutation replay is attempted.

The main log admits at most **1 MiB of serialized UTF-8 JSON per record**
(excluding its terminating LF) and **32 MiB of physical bytes per run**
(including LF and blank lines). Serialization and line reading are bounded while
they happen. Admission checks include any delimiter needed for a previously
unterminated record and happen before writing bytes. An append failure disables
further appends on that writer and preserves the original cause. A rejected
mutation intent therefore prevents dispatch; a failed result append leaves the
existing intent unresolved, so recovery cannot replay the mutation as safe.
There is no reserved space for terminal records: exhaustion can prevent a final
event, and the bridge reports the worker failure separately.

Recovery, event tailing and status/report reads validate the whole journal by
streaming records. Status retains only the sequence; report lookup retains only
the latest relevant transition; tailing retains the requested records. Recovery
keeps the intent/result state needed for its existing safety checks. Reads still
scan the file, and the UI can retain the requested event text; this is bounded
storage and streaming validation, not an incremental file-index cache. A read
during an incomplete append can fail transiently; observation retries without
advancing its cursor. Malformed complete records are never skipped.

The same limits apply to older journals. Oversized logs remain byte-for-byte
intact and cannot resume or supply event/status details through the bounded
reader; history summaries read saved metadata separately. No rotation, automatic
deletion, truncation or loss of recovery records is used to make a run fit.

## 8. Storage layout (Android app filesDir)

```
trollm/
  flows/<sha256>.flow.yaml  (+ .sig — tech debt)
  tasks/<sha256>.tasks.yaml (+ .sig)
  models/coder.gguf .  reasoner.gguf  (+ .sha256 manifests)
  runs/<uuid>/{events.jsonl, state.snapshot.json, artifacts/}
  cache/mcp/...
```

This tree lives in internal `filesDir`, not Android's evictable `cacheDir`.
Downloaded models use `getExternalFilesDir("models")/<sha256>.gguf`; partial
transfers use the same persistent directory with a `.download` suffix. The
internal `models/<slot>.gguf` paths above remain supported for legacy installs.
Run records, configuration and artifacts stay under internal `filesDir/trollm`.
App-specific files survive ordinary process death and APK updates; uninstalling
or clearing app data removes them. External storage can become unavailable and
must produce an access error rather than silently selecting different weights.

Flow/task distribution: downloaded bundle = {flow, registry, prompt files, manifest
with sha256s}; files are verified before install and retained for offline access.
Model identities come from the pinned setup descriptor/effective run configuration,
or the legacy model sidecar manifests, and are verified separately from the bundle.

## 9. Crate contracts (pin these signatures)

### trollm-models

```rust
pub enum FinishReason { Stop, Length, GrammarReject, Timeout }
pub struct GenerateRequest { pub prompt: String, pub grammar_gbnf: Option<String>,
                             pub max_tokens: u32, pub stop: Vec<String>,
                             pub max_duration: Option<std::time::Duration> }
pub struct GenerationCapacity { pub context_tokens: u32, pub context_remaining: u32,
                                pub effective_max_tokens: u32 }
pub struct GenerateResponse { pub text: String, pub tokens_in: u32,
                              pub tokens_out: u32, pub finish: FinishReason,
                              pub capacity: Option<GenerationCapacity>,
                              pub diagnostics: Option<GenerationDiagnostics> }
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
pub enum Provenance { Live, Cache, ServedFromFile, Local }
pub trait ToolGateway: Send {
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>>;
    // Optional model guidance: local/cached descriptors only, no network or waits.
    fn available_tools(&self, grants: &[String]) -> Vec<ToolDescriptor>;
    fn argument_constraints(&self, server: &str, tool: &str) -> Option<serde_json::Value>;
    fn set_deadline(&mut self, deadline: Option<std::time::Instant>);
    fn tool_class(&self, server: &str, tool: &str) -> ToolClass; // stable effective policy
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
`tail_events(run_id, from_seq)`. It also owns worker controls, model preparation
and persisted Android run configuration. `poll_status` exposes the durable phase
alongside `worker_active` and optional ephemeral `activity` (starting, verifying
a model, loading a model, generating). These fields describe this process;
they are not synthetic journal events or evidence of task completion.

## 10. Android app

- Kotlin + Jetpack Compose. Min SDK 33 (Android 13).
- UniFFI bindings from `trollm-uniffi` (generated, not handwritten).
- Rust worker threads execute runs. A **foreground service** observes active work,
  holds a bounded partial wake lock and displays current progress. It releases
  its observer and wake lock when all attached runs pause or finish. Resume
  restarts observation. Foreground execution supports background work; it does
  not guarantee exemption from Doze or process reclamation.
- Secrets (MCP bearer tokens) in Android Keystore-backed EncryptedSharedPreferences;
  TLS-only egress to plan-declared MCP server hosts (network security config allowlist).
- Model downloads are size/SHA-256 verified before publication. Native model
  construction independently rechecks SHA-256 during run-scoped preparation or
  first use of a lazy slot. Opening
  the app, history, or setup does not load models; there is no boot-time preload.
- App never parses prose. It renders durable event entries and current worker
  activity. Notification permission is requested on the first user Start/Resume;
  denial does not block execution and is explained in the app.

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
   Gateway artifact bytes and foreach worklist snapshots charge their owning frame
   and the aggregate; parent/sibling bytes do not consume a child's allowance.
   Artifact limits are checked after writing, with `value` and `limit` expressed
   in bytes in `budget_exhausted` events. The earliest active-frame deadline also
   bounds model generation, MCP discovery/initialization, live calls and pagination.
   The runtime rechecks after generation, before dispatch, after recording returned
   tool outcomes, and before successful transitions/end nodes. A late mutation's
   existing result artifact and returned journal linkage are retained before
   `onLimit`; tools without a declared result artifact retain the journal linkage,
   rather than a new raw payload artifact. A failed late mutation also retains
   `lastError.mutationOutcome: unknown`, including an authored `respond_partial`
   outcome; partial completion does not establish the write's real-world result.
   In-flight native decode operations and
   schema validation remain cooperative work, rather than hard preemption.
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
7. **Mutation journaling wiring**: core writes its intent before gateway dispatch;
   proxy events are separate observations. Required gateway classification is
   forwarded through adapters and checked with root policies. Unresolved intents
   fail closed on resume, including legacy `refetch-state`; actual per-tool
   reconciliation remains future work. Mutation-grant and intent-log tasks cannot
   enable automatic `onError.retry`.

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
   log only, so `cache_hit`/`fetch_progress` don't reach the UI yet. Opening the
   required sidecar validates its records and continues its positive increasing
   sequence (gaps allowed). Duplicate/decreasing sequences, corrupt records and
   write/flush failures stop the worker or dispatch; replacing a failed sink does
   not reset that gateway's failure. Legacy sidecars whose sequence restarted
   cannot resume until repaired deliberately; completed history remains readable
   through the main log. Records are bounded to 1 MiB on scan and append. A valid
   last record without a newline is delimited before appending. Remote `isError`
   details are omitted from persistent errors, retaining only bounded escaped
   tool context and the error category. **Specified future fix**:
   core exposes a shared sequenced sink (`Arc<Mutex<EventLog>>`) that the gateway writes
   through; sidecar is retired.
4. **Cooperative run controls.** The bridge injects an `ExecutionControl` hook
   at dispatch, model/tool and retry boundaries. Retry delays use the existing
   condition variable: Pause wakes and holds the delay, Resume continues its
   remaining duration, and Cancel wakes it without retrying. Cancellation bypasses
   authored `onError` routes and records a durable cancelled outcome. A successful
   mutation's result is journaled before honoring post-call cancellation; journal
   or snapshot failures remain errors. Running model/tool calls remain synchronous
   and cooperative. A user pause is still an in-process hold, not a persisted
   authored pause; process loss requires explicit recovery.
5. **`user_input` persistence**: not in `RunState`; wrapper stores
   `runs/<uuid>/user_input.txt` sidecar so cold resume re-renders prompts faithfully.
   Additive; a later core version may absorb it into RunState.
6. **`sourceBaseline.sha256` semantics** ("authored against" what, exactly) remain undefined
   by design for now; wrapper verifies registry+weights against the *bundle manifest* (§8)
   and deliberately does not enforce plan↔registry equality.
7. **Final report access**: §9 has no result accessor; on `Succeeded`, `poll_status` derives
   `final_report` from the run's own records (terminal node's bound prose outputs). A core
   `run_result()` accessor remains a possible later API.

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
   command. Android persists/discovers run UUIDs and distinguishes a live worker
   from interrupted saved progress. Cold Resume is explicit; overlapping Resume
   requests are serialized before reconstruction. The foreground service retains
   ownership only while attached runs are actively running. Native library packaging and device
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
`configuration.json`; cold resume reloads them. After explicit Start/Resume and
executor reconstruction, up to two task-referenced slots verify and load in
parallel, with coder scheduled first. Plans referencing more than two distinct
slots retain all-slot lazy loading to bound concurrent preparation. A referenced
slot may be prepared even if the chosen route never uses it; an unused slot's
preparation failure does not abort unrelated work. Checksum verification always
precedes construction and recorded sampling settings still apply. See
[model preparation](MODEL-PREPARATION.md) for worker ownership, controls and limits.
Loaded slots are reused within that executor and
released when it terminates; a reconstructed executor verifies and loads again.
Pausing does not proactively evict a loaded model, and ordinary backgrounding
does not launch a second worker or reload a model still owned by the first.
New native runs using legacy
slot files also capture their settings; old snapshots remain readable.
Worker startup and executor persistence failures persist in `worker-failure.json`, and successful executor
reconstruction clears an obsolete failure. The UI defaults to one live run;
the core still supports explicit concurrent callers.

The in-process registry retains live workers and at most 32 ordinary inactive
handles, evicting the oldest registered inactive entries during registry access.
Status and history fall back to saved files; eviction deletes no run data or
models. A failure whose diagnostic could not be written remains in memory outside
that bound, so cache cleanup cannot discard its only error detail. Completed
snapshots never reconstruct on Resume. Explicit Resume of unfinished saved
progress can repair a worker failure consistently before or after handle eviction;
the existing journal safeguards still apply. Failures without a saved snapshot
require a new run, and an unwritten failure remains visible until process exit.

`run_summary(run_id)` reads a small projection of the snapshot and worker failure
record for history, without scanning events or loading models. The list shows
flow title/id, start time and current outcome, with the UUID retained; known
times sort newest first and missing metadata stays accessible. Active rows refresh
only while the screen is started. API errors are classified at their bundle,
runtime or filesystem operation boundary, not by words in paths or messages.

After process loss, a persisted running/paused phase has `worker_active: false`.
The UI shows interrupted progress with Resume available and disables live-only
Pause/Cancel; merely reopening a run performs no inference or tool dispatch.
An interruption before the first saved snapshot is reported as an explicit
startup failure requiring a new run. Resume retains the existing journal safety
checks, including refusal to repeat uncertain writes. Doze/suspension does not
itself imply model destruction; CPU/network resumption and process-death recovery
are separate device checks. See [Android's power-management guidance](https://developer.android.com/training/monitoring-device-state/doze-standby).

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
resume reuses those identities. Older runs without this file cannot silently
adopt a newly signed-in OAuth account. Manual tokens are now stored with their
canonical HTTPS endpoint; the callback receives both name and endpoint and
checks mode, endpoint and token atomically under the connection-edit lock.
Name-only legacy tokens remain stored but inactive until explicitly re-saved
with their intended endpoint. A changed flow endpoint cannot adopt that secret.
Android and Rust share endpoint-identity fixtures. Authentication normalizes
scheme and host case, numeric ports, the default HTTPS port and an empty root
path. It preserves the original path, query and IPv6 spelling, including dot
segments, percent-escape case and an empty query. Transport URL parsing remains
separate; it must not silently choose a different saved credential identity.
An older run whose saved endpoint was rewritten by Rust can no longer resume
when that pin differs from the configured identity. Its files stay intact;
start a new run after checking the connection. There is no alias fallback or
automatic account rebinding. Ordinary unchanged endpoint pins still resume.
Logout, failed sign-in and pending sign-in do not reactivate a retained manual
token; explicitly saving a manual token selects that mode and invalidates the
old grant. Automatic transport/task retries cannot replay declared mutations.

Sign-in captures ownership immediately after the pending grant is stored,
before a cancellable dispatcher return. Callback cleanup also covers its first
credential read. Cleanup runs despite coroutine cancellation and atomically
removes only the same still-pending grant; a completed grant or replacement
login survives delayed cleanup. A cleanup failure does not replace the primary
cancellation, and cancellation during cleanup remains cancellation.

Refresh requests are coalesced by connection, canonical endpoint and grant ID.
The persistence executor reads the latest state, saves SDK state (including
failed-refresh state) before completing the token result and checks the exact
grant again before returning it. An individual timeout does not release an
unfinished SDK request into a second refresh using the old rotated token.
Storage failure, logout or a replaced grant withholds the token. The native
callback still returns a token or no token, without substituting credentials
or retrying a remote action.

Connections and sign-in feedback use fixed network, timeout, cancellation,
reconnect, storage, configuration and generic failure messages. Provider text,
response bodies and raw exception causes are not copied into those messages.
Refresh diagnostics are process-local, limited to 32 exact-grant entries and
fenced by refresh identity so older waiters cannot overwrite newer status.
They retain no token or serialized SDK state; encrypted credentials remain the
source of truth after process loss. Real provider sign-in, refresh and matching
device acceptance remain to be exercised on the compiled APK.


Tool response size accounting uses the existing local proxy and core journals; it sends no analytics. Proxy
`response_size` observations identify server, tool, run-local dispatch number,
full argument hash, stage and source. `compact_json_utf8_bytes` counts compact
JSON for the canonical parsed payload, selecting structured content once when
MCP also echoes it as text. These are UTF-8 bytes, not HTTP wire bytes or tokens.
`raw_payload`, `raw_page`, `complete_collection` (the whole item array),
`selected_page` and `gateway_result` describe nonadditive boundaries. Live,
cache, file-served and local results are measured. Received tool errors retain
numeric canonical payload size and MCP method origin with diagnostics redacted.
`failed_result` has null size when no parsed result returns; unavailable,
partial or malformed responses have no invented zero. Invalid preflight calls
retain the existing no-dispatch, no-proxy-audit behavior.

Core `tool_call_result.gatewayResultCompactJsonUtf8Bytes` measures the shaped
payload. `toolResultContextCandidateUtf8Bytes` measures its encoded continuation
wrapper; cancellation or a limit may prevent appending that candidate. Tool-result
completion has zero candidate bytes. Runtime `flow.choose` records source
`runtime_control`, null payload size and zero candidate bytes. On verified node
exit, `outputBindingSizes` records each artifact binding's source reference,
`artifactFileBytes`, `viewApplied` and `boundCompactJsonUtf8Bytes`. The last field
measures the value bound after `fromArtifact.view`; it does not assert that a
later prompt used it. Numeric size metadata contains no result contents. Raw
artifacts remain preserved independently.

Generation follows the task completion contract: `tool-result` and `tool-or-route` use one tool-call
fence, `flow-route` uses tool calls, and a route-only task uses one call with a
`route` from that node's declared exits. Ordinary `model-prose` tasks retain prose
and permitted tool batches. Tool-free reasoners have no tool grammar. Decoding
constraints guide generation; post-generation checks and the original gateway
schema remain authoritative. Guidance uses only granted local tools and already
cached remote descriptors; it never performs network discovery. Before the first
remote dispatch, explicit task fields and setup requirements still apply, while
unknown wire shapes use the conservative grammar. Dispatch discovers and validates
the selected remote tool normally. Runtime-bound fields are omitted from the model's
argument schema, while required unbound fields and explicit `modelFields` are
retained. Complex schemas retain conservative fallback and final wire validation.

Model-call events retain their existing outer `ms`, slot, token counts and finish
reason. `requestPromptSha256` and nullable `grammarSha256` identify the exact
request strings supplied to the model adapter, not native chat serialization;
these digests log no additional content. Optional numeric `diagnostics` subdivides monotonic wall-clock time into
`weightVerifyMs` and `loadMs` when a slot verifies/loads its weights. With
`preparationAsync: true`, these intervals can overlap other calls and are not
subdivisions of the consuming call's outer duration. `preparationWaitMs` measures
only that call's wait (also reported if it times out before preparation finishes).
Lazy synchronous loading retains its previous accounting. The native
`prepareMs`, `prefillMs` and `generationMs` phases are disjoint. Preparation includes
fresh context creation, chat formatting, tokenization and sampler/grammar setup.
Prefill spans prompt batch construction and all prompt decode calls; generation
spans sampling, output token decoding and stop checks, ending before normal
response assembly/context teardown. These are wall times, not CPU times; verification includes any pause within its
chunked read/check loop. Wrapper
holds, teardown and rounding can leave outer time unattributed; phases must not be
added to outer `ms`. Each phase is nullable: null means unentered/unmeasured, while
zero means a measured interval shorter than one millisecond. Warm calls do not
re-report earlier verification/load costs. Existing token-count semantics remain
unchanged (including full tokenized prompt size after a partial prefill timeout).

Native diagnostic settings identify the engine and configured context/thread
counts. Batch/microbatch sizes are read from the established native context.
Configured thread counts do not establish effective worker utilization; an unknown
device/backend is null. Diagnostics change no sampling, threading, batching,
cache or model settings. Length/Timeout responses retain their partial timings
and existing bounded private response evidence. Genuine errors retain available
partial timings and measured outer duration in the existing private diagnostic,
with unavailable token counts/finish null. A numeric `model_call` observation
with `failed: true` now records the attempted call with those null fields; it is
not a model response or successful task output. Numeric evidence is available in exported
`events.jsonl` and `model-failure-*.json` artifacts for a separately authorized
phone comparison on the same saved capture/model configuration.

The decoder grammar permits zero to four JSON whitespace characters at each
syntax whitespace site, preventing unbounded indentation from consuming a tool
call's generation budget. Adjacent sites can combine, but remain finite. This
restriction does not change whitespace inside argument strings or ordinary prose,
and the post-generation JSON parser still accepts arbitrary valid JSON whitespace.
It does not bound argument content or guarantee semantic correctness.
The generated rule uses `[ \t\n\r]{0,4}` rather than four independent optional
terms. This follows [llama.cpp's efficient repetition guidance](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md#efficient-optional-repetitions)
to reduce redundant grammar matching without changing the permitted whitespace,
tool grants, argument restrictions or sampler order.
See the [native timing experiment](amazon-generalization/PERFORMANCE.md#native-grammar-experiment-2026-11-03)
for measurements and their host/device limits.

Failures may save bounded `model-failure-*.json` artifacts containing the prompt,
grammar and available partial response. They are private diagnostics, not reports
or successful task output: no truncated or timed-out call is dispatched. Files
are capped at 256 KiB, charged against the run/frame artifact allowances and
created with mode 0600 on Unix. Individual text fields are clipped at UTF-8
boundaries with their original sizes and truncation flags; the event records the
complete diagnostic artifact's hash. Failure to save diagnostics cannot
replace the original failure. Run deletion removes these with the other artifacts;
there is no automatic upload. Sanitize any manually shared run data.

## 18. Bounded repeated workflows and live observations

`local.prepare_worklist` freezes a bounded upstream-prepared context/item
envelope; `local.collection_total` requires complete unique successful numeric
coverage before summing exact decimals. Both are explicitly granted coder tools,
with bound input rather than model-produced arrays. See
[BUILTIN-TOOLS.md](BUILTIN-TOOLS.md) for their input, output and rejection rules.

Optional plan `exports` selects named saved values at child return. Optional
foreach collection `view` uses the same structural field mask as artifact views.
Legacy returns remain unchanged. Explicit returns archive the child's complete
bound node-output map before popping its frame; previously projected values
are not magically restored to full remote payloads. Archives, exports and
collections have independent hard limits. Missing/oversized values stop rather
than truncate. [Child-return contracts](amazon-generalization/G2-CONTRACT.md)
define the shape, persistence boundaries and exact caps.

Android screen/service now use `observe_run(run_id, from_seq)`: a bounded
disposable live view with batch cursor and omission indicator. Cold views
strictly reconstruct once and are bounded/evictable; explicit inactive
`invalidate_observation` requests revalidation. Existing `poll_status` and
`tail_events` still strictly read the authoritative journal. Neither view nor
metrics can authorize dispatch or change conservative recovery.

Optional timing/correlation fields identify worker epoch, dispatch attempt and
call across main/proxy observations. Failure observations do not masquerade as
successful tool results. `run_ended` remains the terminal event. The terminal
attempt measurement ends before its final event append and snapshot; it does
not claim a complete terminal wall time. See the
[measurement contract](amazon-generalization/PERFORMANCE.md) for phase overlap,
unknown/zero coverage and the one-pass offline numeric summary.

### Frame-owned iteration and declared child exits

Current snapshots store `foreachCursors` within each stack frame. The older flat
map in the overview is read only for migration; completed child cursors cannot
be reused by a later invocation. Explicit graph reentry starts a new enumeration.
Pause/Resume without `resumeAt` retains the active snapshot and cursor; an explicit
`resumeAt` re-enters its target as an authored graph transition does. The optional
`foreach.onItemOutcome` map publishes a child-result prefix before taking a
declared exit. It consumes authored end outcomes, never model prose.
[Flow reliability](FLOW-RELIABILITY.md) defines migration, partial collections,
paused exits and the unchanged treatment of execution failures.


### Generic reference lookup and task presentation

Literal substring lookup across authored record fields and opt-in identical repeated
numeric ordering keys are isolated built-in operations. Original objects remain saved;
the tools establish literal membership or numeric order, not semantic suitability.
Their defaults preserve the prior exact-selection and strict-decimal contracts; see
[the built-in tool contracts](BUILTIN-TOOLS.md).

For nodes with routes or a prompt supplement, automatic presentation contains allowed
route names and the full bounded last failure. Mechanical tool guidance comes before
the current authored instructions/evidence. Explicit runtime bindings, graph edges,
mutation journaling and persisted state are unchanged. Unopted legacy prompt assembly
is unchanged. Recovery keeps the failure origin and mutation outcome across nodes;
see [adaptive flows](ADAPTIVE-FLOWS.md#step-instructions-and-recovery-context).
