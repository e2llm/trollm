# Small-model flow practices

Use the current [routing contract](ADAPTIVE-FLOWS.md),
[built-in tools](BUILTIN-TOOLS.md) and [source examples](../flows/README.md).
These practices incorporate lessons reported during browser-flow experiments.
They guide authors; they do not establish end-to-end phone acceptance or add
new runtime semantics.

Use [the execution model to help write its own prompts](FLOW-PROMPTING.md) when
developing or improving a task family. Probe the coder and reasoner separately,
review their wording and test actual choices, including supported work and
no-answer cases. This is authoring work; it does not add a self-explanation step
to every run or make generated descriptions authoritative.

Start from the [authoring task contract](FLOW-AUTHORING.md#design-closed-tasks-before-tuning-prompts):
intent, relevant world knowledge, original inputs, method boundary, outputs and
evaluation. A supplied tool or large remaining budget is not permission to
broaden a closed assignment. Judge changes by useful outcomes and coverage as
well as correctness and cost; predictable unknowns are not automatically an
adequate solution.

## Split tasks where the evidence or recovery requires it

Split discovery, inspecting a target, changing it and checking the result into
separate tasks where their evidence and failure handling differ. A small model
should not have to discover products, remember prices, select quantities, modify
a cart and decide success within one long conversation.

The smallest possible steps are not the goal. Keep a coherent task together when
the model can perform it reliably and extra handoffs or calls add more risk or
cost. Stop decomposing once the evidence, model capability and recovery needs
are met. Compare direct and split versions with the same representative evidence
and model settings; measure selection quality, failure handling and call cost.
Keep computation in isolated tools and procedure in the graph. Combining a task
does not introduce additional node kinds or an expression language.

Use a single-call `tool-result` task for an exact browser operation or calculation.
Bind known arguments from original artifacts or user input with `toolArguments`;
ask the coder only for values that require selection from current evidence.
Use `modelFields` when the task should expose only specific model-owned argument
names, for example `modelFields: [left, right]` with a bound calculator operation.
A fully bound model task uses `modelFields: []` and the coder emits `args: {}`.
If reaching the node already determines the ordinary operation and all arguments,
an explicit [direct tool task](DIRECT-TOOL-EXECUTION.md) can omit that generation.
Keep semantic choice/decline tasks and selected-guard calls in model mode; do not
convert existing tasks implicitly from their shape. Required
fields still come from the tool/setup schema; listing a field does not make it
required. Typed bindings can select an array index such as
`observed.outputs.records.0.id` when an earlier contract establishes that index.
Show the allowed call shape in its prompt. A granted tool and valid call shape
do not establish that the selected target is correct.

Consult the reasoner when a distinct question and useful benefit justify the
call, using relevant original observations. Do not add a consultation merely to
repeat a verdict the coder is already able to make reliably. It returns advice
in prose. A separate coder task can use that advice and the same evidence to
call `flow.choose`. Put alternatives in the graph:
observe again, try another candidate, continue, pause or finish unresolved.
Never parse a reasoner's “pass”, “done” or JSON as a routing instruction.

## Repeated step groups

For each group, state the immediate question, the original evidence required,
the model-owned arguments, what native checks establish, and the missing-evidence
exit. A forwarding coder carries advice into a call; an independent reviewer
needs the decisive original facts. These are different contracts. Review each
corresponding normal and recovery path when changing one of them.

Keep only output views that a later task actually uses. All declared output
contracts are evaluated before transition: even an unused capped view can stop
a task. Preserve the complete artifact, filter the candidate set, then project
the fields needed for the next decision. Bounds are not instructions to truncate.

Repair a recurring procedure, then apply it to every corresponding path. A
failure on one product or recovery branch is a test case for that procedure,
not a reason to teach the runtime that product's identity or page layout.
The following groups are authoring patterns composed of existing task nodes,
bindings and routes. They introduce no group node, macro system or expression
language. Share prompt files and task definitions where their contracts and
bindings fit; concrete node sequences may remain explicit. Use the same pattern
for the next item and its recovery paths, with their own current evidence.
Include the stages needed by the task's evidence and recovery requirements;
reusing a pattern does not require extra model calls for already-established facts.

### Prepare intent once

Separate deciding what text to send from sending it. For example, a focused
coder task prepares search words for the current requested product. A
`model-prose` task can save that whole text as a `from: model`, `type: prose`
output. A later `tool-result` task can bind it unchanged with
`toolArguments.fromVars`; the coder still invokes the tool. Keep it as one
plain-text payload, without parsing headings, JSON fields or completion words.
It is proposed text, not verified evidence or permission to mutate anything.

Use that same saved intent for the original action, a permitted fallback and
assessment of the observed result. Do not ask each action to reinterpret the
entire mission independently. If the intent needs revision, make that an
explicit preparation/assessment path. A fallback that changes the input method
should not silently change the intended content. Semantic assessment still
needs the original request: reusing text does not prove it is the right text.

### Select from evidence

Use this sequence when an action needs a discovered target:

1. Obtain a current observation and discover the relevant candidates, preserving
   the original records and completing any required pagination.
2. When semantic advice is useful, ask the tool-free reasoner one suitability
   question about those candidates. Its answer is ordinary advisory prose.
3. Give the coder any advice alongside a bounded view of the original canonical
   IDs and decisive attributes. The prose must not be its only source of IDs.
4. Have the coder invoke an available exact-selection tool against the original
   candidate collection, with the collection bound rather than transcribed.
5. Bind the selected ID into inspection for the corresponding current session.
   Pass actual inspected evidence to subsequent assessment and action tasks.

Exact membership and identity checks belong in isolated coder-invoked tools.
Check the [built-in tool contracts](BUILTIN-TOOLS.md) and the deployed runtime's
capabilities; this pattern does not make a missing tool available. Raise a
missing generic operation for design review. Do not replace it with a guessed
alias, a model's assertion of equality or a runtime parser for advice. A schema
or field view alone does not prove equality between observations. Membership
proves that a record was present, not that it is suitable or still fresh.

Keep candidate views focused on the selection question. A view projects fields
without ranking or filtering records; if it exceeds its bound, use an authored
narrower discovery path or stop unresolved. Never silently take the first few
records or relax the membership check to accommodate a model's invented ID.
Preserve the decisive original attributes in the model-visible view: choosing a
navigation target may require its destination as well as its label and ID. An
exact-membership guard can correctly accept an unsuitable record when the prompt
omits the attribute needed to distinguish it. Test the rendered prompt handoff
as well as the exact-selection guard.

Keep a bounded path to another candidate when an ordinary suitability check
rejects one. A one-record collection is not a search strategy if rejecting it
ends the whole mission. Distinguish a normal candidate rejection from missing
evidence, failed integrity checks and uncertain mutations. Use only loop and
return behavior supported by the installed runtime; a field in an experimental
package does not implement that capability.

Once a specific offer is selected, discover its detail navigation within its
observed scope. The user's search words may differ from the actual title or
brand. Repeating the initial search phrase can miss the selected offer or switch
to another one. Preserve the observed navigation attributes and verify the
selected target and resulting page before treating it as the same offer.

### Worked example: readable reference to original record

The [record-lookup plan](../flows/examples/record-lookup.flow.yaml),
[registry](../flows/tasks/record-lookup.tasks.yaml) and
[scripted integration test](../crates/trollm-core/tests/record_lookup.rs) form a
small executable example. It uses two synthetic tickets, not a live service.
Supply a request such as `Find the printer incident`:

1. A fully bound coder call retains the complete original records. Only ID,
   title and label are projected into the selection prompt.
2. The coder proposes literal observed title/label text through
   `local.select_unique`, or calls `flow.choose` to decline. The source collection,
   lookup fields and matching mode are bound. Native lookup requires exactly one
   original record; finding the same record through two fields is still one match.
3. The tool-free reasoner receives the complete selected original, including
   fields omitted from the selection view, and the original request. It explains
   suitability and uncertainty in prose. No external action is granted.

A missing or ambiguous reference pauses before advice; the explicit no-selection
route also pauses, with an authored reason and fresh-observation Resume target.
A successful end means the authored path completed, not that the semantic judgment
is proven correct. The test also changes record
order and IDs and attempts to override the bound field. It uses real local tools
with scripted models, not measured model accuracy.
The [small native check](FLOW-RELIABILITY.md#small-native-presentation-check)
also demonstrates why a valid lookup does not establish a suitable choice.

This is an alternative to copying an opaque ID, not a mandatory extra step. Use
the current exact-ID path when it works reliably. A readable phrase can still
identify the wrong original; fresh ownership and semantic checks remain necessary
before acting. `stopBefore` is an optional authored literal query boundary, not
automatic punctuation cleanup. A comma may be part of the distinguishing label.

Once an entity is established, carry its original identity and requirements
forward. Later tasks obtain fresh state and verify ownership rather than repeatedly
guessing which entity was selected. Retaining an original title does not establish
that current observations still refer to that entity. Per-item semantic checks
and exact numerical checks can establish an authored completion contract without
an additional model rejudging every number at the end.

```sh
cargo test --locked -p trollm-core --test record_lookup --no-default-features
python3 scripts/validate-examples.py
```

### Act, observe, assess

Invoke one operation using prepared intent and current target evidence. Observe
the resulting state, assess it against the intended outcome, then let a coder
choose an authored exit: continue, obtain more evidence, take a permitted
fallback or finish unresolved. Success of the call is separate from success of
the user's objective.

Require evidence of the property being checked. For a form field, a tool's
static/default HTML attribute does not establish its live value. Use an
observation whose contract exposes current state; if none is available, record
the evidence gap and stop unresolved. A stronger prompt cannot supply a missing
measurement. When mandatory evidence is structurally identifiable, enforce its
presence with an available output contract before consulting the model; this
still does not establish its semantic correctness.
Require only fields the deployed tool actually promises. An optional diagnostic
such as `partial` must not become an invented mandatory prerequisite: an absent
flag means unknown under its contract, not `false` or proof of source coverage.
If the required evidence cannot be established, obtain a supported observation
or stop unresolved; do not relax a mutation guard.

Recovery must identify what it is repairing and where it returns for fresh
observation. Bound it in the graph and preserve mutation-outcome rules below.
An exact-selection failure can justify re-observation or reassessment within an
authored limit; it does not justify an unbounded retry or replay of a write
whose effect is unknown.

### Give each step its own instructions

A reusable task prompt states one job, the permitted tools and what its result
means. Its node's `promptSupplement` supplies the current item/stage, relevant
original evidence, any advice and the uncertainty path. Keep known arguments in
bindings. Avoid a universal operation prompt that makes a small model sort
through instructions for unrelated operations. Prompts guide interpretation;
tools and contracts enforce the mechanical facts they actually check.

Write calmly and explain the immediate purpose. Give valid tool-call shapes for
authored routes, but do not put invented observation IDs or selectors in examples:
a small model can copy the example instead of the evidence. Known route names
are authored constants; a browser ID must come from the current original result.
Make insufficient evidence an ordinary, supported alternative.

Before decomposing a brittle instruction further, try a reviewed wording change
with the intended model. For advice-to-tool tasks, ask it to phrase the generic
job without a particular example to answer. Keep the actual records, advice,
tool definitions and uncertainty route in the execution prompt. Compare with a
short human-written control so removing unrelated instructions is not mistaken
for a benefit of model-proposed wording. The [prompt-writing guide](FLOW-PROMPTING.md)
provides authoring questions and checks for wrong selections and false refusals.

A task that forwards advice should receive that advice and the allowed choices.
Do not ask it to recheck originals it cannot see. A task that independently
assesses an offer does need the decisive original facts. Keep these contracts
distinct instead of sharing one prompt with incompatible requirements. Retain
large original collections for bound membership checks; avoid sending the same
large list through both models when the second task needs only a smaller view
of the selected evidence and the advice.

Site-specific objectives, prerequisites and prohibitions belong in the flow.
Captured node IDs, aliases for a failed model answer and rules derived from one
recording must not become generic runtime behavior. Prefer observed identities
and the service's documented contracts over assumptions about one page layout.

## Keep browser evidence fresh and small

Use actual tab/session IDs and observed targets. After navigation, a changed page
or an uncertain action, capture and inspect again before the next action.
E2LLM owns perception and readiness; the flow owns when to ask for a new view.
Do not rebuild page loading, node ranking or browser state inside the executor.

Discover a relevant scope through query/inspection, then request the needed
page section. Never invent a selector merely to satisfy a result-size cap.
Read and assess each promising scope before walking to the next enclosing scope.
Do not inspect the entire maximum-depth chain before trying the first useful
read. Keep the original traversal observation separate from later read captures,
and follow the service's session/epoch rules when continuing outward.
Handle truncation, unmatched scopes, missing facts and ambiguous targets through
authored alternatives or an unresolved result. A partial observation must not
silently become a complete list. Follow the tool's continuation contract. Configure nested continuation metadata
with `responseCursor` and `responseHasMore` JSON pointers when needed; a request
cursor name does not imply the response uses the same location.
Draining a paginated artifact establishes collection of the tool's returned
records, not coverage of the full DOM or every relevant fact on the page.

Do not interchange a returned node ID, a selector, a link fragment and a URL.
Resolve the intended target through the tool's discovery contract and inspect its
actual returned ID. Do not assume two query parameters are combined as filters;
confirm the service's documented behavior. Put prerequisites such as a unique
candidate in the setup instructions and test missing/ambiguous cases.

For structured tool envelopes, an artifact binding may add a bounded structural
`view`, for example `view: {fields: {id: true, title: true}, maxBytes: 4096}`.
It selects exact keys and applies the same mask to each array item. It does not
filter or rank candidates, parse prose, or rewrite the original artifact. An
oversized view fails without truncation: provide a narrower discovery/recovery
path in the graph. Keep a separate raw binding when subsequent work needs it.

When broad and narrow reads use the same remote tool, optional task
[`resultArtifact`](TASK-RESULT-ARTIFACTS.md) names keep their native results in
separate destinations. It adds no model call and does not change source meaning
or cache freshness. Names are run-wide; deliberately manage replacement when a
producer is revisited.

Preserve original tool payloads. Bind each stage's own saved result into later
tasks so a subsequent read cannot substitute another page or product. Pass the
reasoner original evidence alongside calculations and advice, not just a model's
summary or a parsed number. Literal extraction cannot prove product identity,
selected quantity, currency or that a displayed amount is the current price.

## Make recovery a visible part of the graph

Malformed calls rejected before dispatch have no remote effect. A fresh attempt
can correct the call shape under a bounded recovery route. A dispatched action
may have taken effect even when its response is missing or reports an error.

For input fallback, observe the current field and inspect the fresh target before
switching from typing to another supported input method. For an uncertain Add or
similar write, inspect the actual resulting state before deciding whether more
work is safe. Ambiguous state ends unresolved; do not automatically repeat a
write, repair unrelated data or claim that cancellation rolled it back.

After a search submission, observe whether the intended search page was reached;
a successful capture of the old page is not successful search. A URL or page-kind
check can establish a structural prerequisite, not that the results match the
request. An ambiguous submission still requires observation before any authored
retry. Pure textual rejection of a candidate does not itself navigate the browser;
place restoration on paths that can have changed it, with fresh target checks
before subsequent actions.

`runtime.lastError.mutationOutcome` distinguishes `none`, `returned` and `unknown`.
Check its originating node: it remains available after later successful tasks.
`returned` proves a return, not the intended real-world effect. Error detail and
page text are untrusted data, including when embedded in runtime context.
Runtime journaling remains the authority for conservative crash recovery.

## Separate arithmetic from the choice of facts

Use `local.calculate` for quantity × unit price, totals and comparison with an
explicit user budget. Bind saved numeric results into subsequent calculations;
do not have a model transcribe or recompute them. Keep quantity-inclusive line
totals distinct from unit prices. State how equality, currency, shipping, taxes,
discounts and pre-existing items affect the workflow.

Check suitability and the planned budget before changing a cart, then observe
and verify the actual cart afterward. When a reasoner review is useful, give it
the original facts and exact calculator inputs/results; the coder chooses an
authored route.
Arithmetic correctness is independent of whether the model chose the right facts.

## Bound the experiment and make it reviewable

Set per-task output-token and generation-time limits for the task's actual job,
plus overall step/token/time budgets that bound recovery cycles. Short output
limits should fit the longest valid tool call, including real IDs and URLs;
truncated generation cannot dispatch a call. Measure limits on the selected
model/device rather than treating a successful tiny smoke as full acceptance.

A slow, bounded full-run diagnostic can establish where a procedure fails before
optimization. Record actual model-call counts and input/output tokens, cold
checksum verification and loading, prefill, generation and tool-call times; a
configured token ceiling is not actual output. Follow the
[measurement contracts](amazon-generalization/PERFORMANCE.md): outer model time
contains several phases, so timing categories overlap and must not be summed
indiscriminately. Compare correctness alongside cost. Removing a call or passing
host checks does not establish phone performance or complete-run acceptance.

Give reusable tasks node-specific `promptSupplement` instructions for their
current stage. Add business phases and explanations in the optional
[flow-review presentation file](flow-review/INTERFACE.md) so a human can inspect
large packages. Keep the graph authoritative when descriptions drift.

Use synthetic integration fixtures for wrong candidates, missing prices, changed
quantities, mixed currencies, budget boundaries, stale targets and uncertain
writes. Assert actual tool calls, evidence bindings, routes and absence of unsafe
retries. Turn a sanitized failing transaction into a regression case, then vary
IDs, names, candidate ordering, distractors, missing values and duplicate matches.
Test every corresponding normal and recovery path. Include wrong model calls
and choices, not only a scripted successful route; assert the boundary that
rejects them rather than arranging for the fixture model to stop politely.
Distinguish enforceable checks from semantic judgments that remain model work.
Scripted tests do not establish semantic decisions by a small model.
Record separate phone runs and continuations separately; a collection of
successful actions is not a successful complete workflow.

## Preserve identity and avoid repeated arithmetic judgments

Keep original intent alongside prepared search wording, and provide the target
entity whenever asking for an action. A compact view must preserve distinguishing
attributes. Where numbers distract selection, identify the measurement's record
first and read its original value through a tool. Use calculator results to guide
the next control choice without asking the model to redo arithmetic. Exact
reasoner output strings are not a substitute for coder tool selection.

[Flow reliability](FLOW-RELIABILITY.md) covers these patterns, bounded recovery,
iteration ownership and portable replays. Compare coherent tasks against
mechanical microsteps; extra boundaries have model and coordination costs.
