# Small-model flow practices

Use the current [routing contract](ADAPTIVE-FLOWS.md),
[built-in tools](BUILTIN-TOOLS.md) and [source examples](../flows/README.md).
These practices incorporate lessons reported during browser-flow experiments.
They guide authors; they do not establish end-to-end phone acceptance or add
new runtime semantics.

## Give each model task one question or operation

Split discovery, inspecting a target, changing it and checking the result into
separate tasks where their evidence and failure handling differ. A small model
should not have to discover products, remember prices, select quantities, modify
a cart and decide success within one long conversation.

Use a single-call `tool-result` task for an exact browser operation or calculation.
Bind known arguments from original artifacts or user input with `toolArguments`;
ask the coder only for values that require selection from current evidence.
Use `modelFields` when the task should expose only specific model-owned argument
names, for example `modelFields: [left, right]` with a bound calculator operation.
A fully bound call uses `modelFields: []` and the coder emits `args: {}`. Required
fields still come from the tool/setup schema; listing a field does not make it
required. Typed bindings can select an array index such as
`observed.outputs.records.0.id` when an earlier contract establishes that index.
Show the allowed call shape in its prompt. A granted tool and valid call shape
do not establish that the selected target is correct.

Consult the reasoner with the relevant original observations and a narrow
question. It returns advice in prose. A separate coder task can use that advice
and the same evidence to call `flow.choose`. Put alternatives in the graph:
observe again, try another candidate, continue, pause or finish unresolved.
Never parse a reasoner's “pass”, “done” or JSON as a routing instruction.

## Keep browser evidence fresh and small

Use actual tab/session IDs and observed targets. After navigation, a changed page
or an uncertain action, capture and inspect again before the next action.
E2LLM owns perception and readiness; the flow owns when to ask for a new view.
Do not rebuild page loading, node ranking or browser state inside the executor.

Discover a relevant scope through query/inspection, then request the needed
page section. Never invent a selector merely to satisfy a result-size cap.
Handle truncation, unmatched scopes, missing facts and ambiguous targets through
authored alternatives or an unresolved result. A partial observation must not
silently become a complete list. Follow the tool's continuation contract. Configure nested continuation metadata
with `responseCursor` and `responseHasMore` JSON pointers when needed; a request
cursor name does not imply the response uses the same location.

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
and verify the actual cart afterward. Have the reasoner review the original
facts and exact calculator inputs/results; the coder chooses an authored route.
Arithmetic correctness is independent of whether the model chose the right facts.

## Bound the experiment and make it reviewable

Set per-task output-token and generation-time limits for the task's actual job,
plus overall step/token/time budgets that bound recovery cycles. Short output
limits should fit the longest valid tool call, including real IDs and URLs;
truncated generation cannot dispatch a call. Measure limits on the selected
model/device rather than treating a successful tiny smoke as full acceptance.

Give reusable tasks node-specific `promptSupplement` instructions for their
current stage. Add business phases and explanations in the optional
[flow-review presentation file](flow-review/INTERFACE.md) so a human can inspect
large packages. Keep the graph authoritative when descriptions drift.

Use synthetic integration fixtures for wrong candidates, missing prices, changed
quantities, mixed currencies, budget boundaries, stale targets and uncertain
writes. Assert actual tool calls, evidence bindings, routes and absence of unsafe
retries. Scripted tests do not establish semantic decisions by a small model.
Record separate phone runs and continuations separately; a collection of
successful actions is not a successful complete workflow.
