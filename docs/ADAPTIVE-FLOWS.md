# Coder-directed flows

Approved implementation contract, 2026-10-23. This source implements the generic
extension; delivery and the separate Amazon track remain tracked in the product
plan and issues. This document is the reading entry point for both tracks.
Older flow bundles retain their current
semantics; older runtimes reject these new fields rather than ignore them.

## Purpose and boundaries

The flow declares the permitted transitions. The coder requests one through a
tool call. The runtime validates, records and applies it at a node boundary.
The reasoner supplies advisory prose and has no tools. A selected route is a
model decision, not independent proof that a real-world objective was achieved.

Keep the graph a coordinator of model tasks. Computation and external work live
in isolated tools invoked by the coder. Do not add model-free tool nodes,
expressions, natural-language parsers or a second programming language to the
flow. No task name, artifact name, product name or website selector changes core
execution semantics. E2LLM owns browser perception/readiness. Authors guide
re-observation and recovery through prompts and graph edges.

## Routing contract

Task registries add `completion: flow-route`. Such a task uses the literal
`coder` slot and explicitly grants `flow.choose`. It may also grant ordinary
tools and use a bounded model loop. Prose never completes it. Model-derived
outputs are forbidden, as for `tool-result`; artifact contracts still apply.
Task-level `toolArguments` requires single-call `tool-result` or `tool-or-route`
completion; the latter is described below. `flow-route` does not inject bound
arguments into its multi-generation tool loop.

A task node using this completion mode declares `routes` instead of
`onVerified`: a nonempty map of at most 16 authored route names to ordinary
transitions (node IDs or inline paused/failed transitions). A paused route must
include an explicit `resumeAt` node ID; the paused snapshot stores that node as
the continuation. Authors can select a fresh observation or other safe next step
instead of resuming the completed chooser or mutation task. The validator checks
that the destination exists; it does not prohibit selecting the chooser itself.
`resumeAt` is valid only on paused transitions and must name an existing node.
Legacy inline pauses without `resumeAt` retain their previous behavior outside
routes. Explicit null is rejected for `routes`, `promptSupplement` and
`resumeAt`. Names use
ASCII identifiers, at most 64 characters. All targets are validated at load.
Only `tool-or-route` also declares routes, together with `onVerified`, as described
below. `onError` keeps its current
meaning and budgets continue to bound cycles.

`flow` is a reserved runtime namespace, never a remote MCP server, setup
connection or gateway policy. The only control tool in this contract is:

```text
<tool_call>{"server":"flow","tool":"choose","args":{"route":"inspect_more"}}</tool_call>
```

Arguments contain exactly one string field, `route`, naming an exit of the
current node. The runtime provides the current allowed names and destinations
as context; the coder does not invent destinations. Unknown tools, unauthorized
routes, missing choices and reasoner grants fail closed.

A generation containing `flow.choose` must contain exactly one tool call.
Reject mixed action/control batches before dispatching any call. A valid choice
ends the task, verifies its output/artifact contract, records the chosen route
and uses the existing durable transition path. It never jumps midway through
a tool batch. Restarting a committed transition must not repeat that decision
or a completed mutation. Existing protections for interrupted mutation tasks
remain in force; this feature does not promise exactly-once remote effects.

```yaml
assess_next:
  kind: task
  taskRef: decide-next
  promptSupplement: |
    Decide whether to inspect further, repair, or write the final report.
    Use the supplied observations and advice. Do not modify the cart here.
  routes:
    inspect_more: observe
    repair: repair
    finish: report
    wait: { status: paused, reason: "Waiting for explicit resume", resumeAt: observe }
```

## Step instructions and recovery context

Task nodes may add `promptSupplement`, an inline template capped at 16 KiB of
UTF-8 bytes. JSON Schema's `maxLength` counts characters; the runtime's byte
check is authoritative and can reject a shorter non-ASCII template.
Rendering uses the existing variable substitution, not expressions. Compose
the registry prompt, then the supplement, then runtime context, with labelled
boundaries. Nodes with either a supplement or routes opt into this context;
legacy nodes without either retain their prompt behavior.

For opted-in nodes, the reserved template variable `runtime` contains the
current `node`, `taskRef`, the declared `routes`, and `lastError` (initially
null). The same bounded context is appended automatically so a routing model
always sees its legal choices. Do not dump the whole run into the prompt.
Labelled sections distinguish authored instructions, runtime context and bound
outputs; they are not a trust boundary for model interpretation. In particular,
`lastError.detail` can contain gateway or remote-service error text and must be
treated as untrusted diagnostic data, never instructions. Templates can bind
the relevant stored outputs and advisory prose without making them authoritative.

`lastError` records the last failed task in this frame: `node`, `class`, a bounded
`detail`, and `mutationOutcome`. Its values are `none` (no mutation dispatched
in that attempt), `returned` (dispatched mutations returned successfully), or
`unknown` (a dispatched mutation has no known successful return). A returned
call is not proof of semantic success. An unknown outcome dominates earlier
successful calls. Store this small record in the frame snapshot, with a default
for old snapshots; retain it until another task failure replaces it. Recovery
prompts must heed its originating node rather than assume it describes the
most recent successful step.

Malformed calls rejected before dispatch can be corrected. A possibly applied
mutation needs observation before another attempt. The flow and coder choose
that recovery; core does not infer a cart state, interpret browser text or
automatically repeat writes. Crash recovery keeps the existing conservative
handling of uncommitted and uncertain mutations.

## Generation limits

Task definitions add optional `maxOutputTokens` (1 through 2048) and
`generationTimeoutS` (1 through 600). Omission preserves the 2048-token ceiling
and no additional per-generation deadline. The remaining run/active-frame
deadline still applies. The native backend checks deadlines
between prefill batches and generated tokens; it cannot preempt one native
decode operation. Existing overall budgets, cancellation and finish-reason
failure behavior remain. Truncated, timed-out or grammar-rejected output cannot dispatch
tools or complete a step. Calls, discovery and pagination use the remaining
run deadline too; returned mutation outcomes are journaled before a late result
ends the run. The runtime never treats a timeout as proof that a write did not
happen. See [the architecture contract](ARCHITECTURE-ANDROID.md) for argument
constraints, compact artifact views and private failure diagnostics.

## Implementation and handoff

1. Generic runtime: schema/readers/validators, routing, prompt/context rendering,
   durable error/transition state, generation limits and focused unit/integration
   tests. Add a non-browser example and preserve existing-flow compatibility.
2. Amazon development: create `e2llm-amazon-dev-1` from clean updated main.
   Re-author discovery, inspection, reasoner advice, coder action/routing,
   cart observation, `local.calculate`, assessment and reporting/recovery.
   Use graph stages and instructions, existing E2LLM tools and exact calculator
   results. Do not import the old branch's domain-specific executor, magic-word
   completion, environment binaries or automatic diagnostic exports.
3. Keep ordinary branch pushes from triggering Android compilation. Validate
   host unit/integration tests and schema/bundle consistency. No live browser,
   simulator or physical-phone end-to-end tests are part of this work package.
   The phone-flow maintainer owns physical acceptance using the new branch.

Main-session ownership covers specification and architecture. Delegate bounded
implementation tasks with distinct file ownership; bring material departures
back for alignment. Keep host RAM/disk use bounded and leave long CI jobs to
finish without polling loops. Track delivery in repository issues and MRs;
this contract and onboarding links survive session changes.

Host regression checks include actual-source
`cargo test --locked -p trollm-core --test adaptive_example`, core control and
recovery tests in `adaptive_routing`, and bridge bundle-admission tests. They use
scripted models and local mock services. A paused routing test performs a mock
mutation before choosing a continuation and checks both warm and cold Resume
without repeating that mutation. No result here establishes real-model route
selection or physical-phone acceptance.

## One tool result or one authored route

`completion: tool-or-route` is a coder-only, single-generation alternative to
`tool-result`. Grant an ordinary tool and `flow.choose`, declare `onVerified`
and `routes`, and keep `loop.maxSteps` at one. An ordinary call uses the existing
typed argument bindings and must satisfy the normal artifact output contract
before `onVerified`. Required artifacts must come from that current call;
effective node output overrides retain their existing schema validation.

A sole `flow.choose` call selects a declared route without producing ordinary
success outputs. Only this completion mode skips those success-artifact
obligations on its control branch. Previous outputs for the node, including bare
and `.outputs` aliases, are cleared on a new attempt and remain absent after
decline and Resume. Original artifact files remain retained as evidence.

The mode forbids model output bindings, bound `flow.choose` arguments, prose
completion and mixed or multiple calls. Tool failures and invalid calls follow
`onError`; they never become semantic abstention. Other completion modes retain
their contracts. A route named `more_evidence` is only an authored exit: it does
not obtain evidence unless the graph explicitly performs that work.

## Declared collection exits

`foreach.onItemOutcome` can leave an enumeration after a child reaches a declared
end outcome and its result has been collected and verified. This is separate
from a coder's `flow.choose`: models cannot invent child outcomes or graph edges.
See [frame-owned iteration and recovery](FLOW-RELIABILITY.md) for the contract,
including explicit Resume continuations and unchanged failure semantics.
