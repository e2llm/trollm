# Count-independent procedure and capability contracts

Historical design implemented for flow105 / `0.1.0-rc.8` / Android code 43.
See [versioning](../VERSIONING.md) and [Android installation](../INSTALL-ANDROID.md)
for current release status. The approved design below started from flow104
(`239e7e6`, in private development history rather than curated GitHub history).
Exact tool arguments are in
[BUILTIN-TOOLS.md](../BUILTIN-TOOLS.md); child returns and limits are in
[G2-CONTRACT.md](G2-CONTRACT.md). See the [scope](README.md).

## 1. Prepared input, not an input product

Assume useful preformatted input. The demonstrator needs original per-item prose,
requested pack quantities, global preferences/currency and an explicit budget.
There is no work package for extracting arbitrary shopping missions or building
a repeatable Android form.

For the implementation fixture, use a small prepared envelope containing a
context string and an item array. Each entry carries original request prose and
a positive integer quantity as a decimal string. An upstream author/wrapper can
prepare this envelope; a model inside the flow is never asked to produce the
whole JSON array. It can travel in the existing text input field as serialized
data. The existing browser and budget fields remain usable. This framing is a
demonstrator adapter, not a new natural-language parsing contract.

The first coder invokes a generic, fully bound local worklist-materialization
tool. It validates the envelope's structural/size limits, assigns stable opaque
request IDs, preserves original entry values/context and publishes an artifact.
Quantity/request-specific validation belongs in the authored result schema.
Tool code must not recognize Amazon, product names or task IDs. A flow already
receiving a suitable MCP array can skip this adapter, as ticket triage does.

Input bounds: 1–8 records; finite per-record and total input byte
limits within the existing 64 KiB bridge limit. No inferred items, silent record
dropping, truncation or automatic duplicate consolidation. The frozen list is
immutable for a run. Changing it starts a new run after observing actual state.
The envelope uses `context` and `items` as specified by `local.prepare_worklist`;
support one format, not a family of parsers.

## 2. Root procedure and reusable children

| Stage | Inputs and responsibility | Required outcome |
| --- | --- | --- |
| Prepare | Bound prepared input, browser and global context | Complete immutable worklist; malformed/empty/oversized input stops before browser changes. |
| Baseline | Fresh original active-cart observation | Preserve unrelated existing contents and quantities. No assumption of an empty cart. |
| Discover each | One original request, quantity and shared constraints | Supported offer, original evidence, exact planned line calculation and advisory suitability. One alternate prospect at most. |
| Basket preflight | Every requested ID and accepted offer, all planned amounts, budget | Complete coverage, distinct allocation, compatible currency/price basis and permitted exact total before any Add. |
| Execute each | One accepted request/offer plus preflight context | Fresh cart and product facts; requested-minus-present; supported quantity; at most one Add attempt; observe result. |
| Final observation | Fresh complete original cart prose | One saved observation for final per-item checks. |
| Reconcile each | One requested identity/quantity and that original observation | Actual matching quantity and exact actual line calculation, or unresolved. |
| Final assessment | All requested IDs, actual totals, budget, baseline/current cart | Report observed results and limitations, or unresolved. No checkout. |

Use three root-level foreach nodes with distinct stable IDs: discovery,
execution and reconciliation. Each child has one reusable node/task/prompt set;
there are no `first_*`, `second_*` or generated per-count copies. Repeated
positions within a bounded recovery branch may remain explicit.

### Discovery

Retain the existing sequence where evidence changes: prepare search text once,
inspect the observed search field, invoke its bound input operation, observe and
verify live field identity/value, submit, observe fresh results, choose a
supported heading, prove exact original membership, inspect canonically,
navigate, read the actual scoped offer and assess suitability.

The heading adviser sees the complete bounded current candidate view, the
original current item and the saved query. The query is proposed search text,
not a replacement for the original request. A rejected offer permits one return
to the actual saved results URL, fresh candidates and an alternate assessment
with the original rejection evidence/advice. A second unsupported offer stops.
No stale ID is carried into a new capture as an action target.

Calculate the planned merchandise line against the requested final quantity.
When matching units already exist in the baseline cart, establish that their
actual price/discount basis agrees with the accepted offer. The initial
demonstrator stops on a different or ambiguous basis; it does not silently
value existing units at a new offer price. Mixed-price allocation is deferred.
Bind that quantity from prepared input when multiplying unit price; if the
original page explicitly gives a quantity-inclusive matching line total, retain
the existing distinct multiplier-one path and semantic assessment. Existing
requested items remain part of the budget even when no additional Add is needed.

### Execution

The simplest safe first implementation obtains a fresh cart observation at the
start of each child. It does not assume foreach provides mutable carry state or
that an earlier sibling's output is automatically available. Revisit the exact
accepted URL and compare fresh price, stock, variant and currency with accepted
facts. Changed or ambiguous offers stop; automatic repricing is out of scope.

Before the next Add, also compare existing and previously added requested lines
in that fresh cart with the compact approved basket. A price/quantity change
between item children invalidates the earlier authorization and stops the run.
Within-child before/after preservation alone cannot establish this. Only this
basket-authorization question needs cross-item context; ordinary target/tool
tasks remain item-local. External page changes cannot be made atomic by a flow.

Use the existing calculator to subtract matching present quantity from requested
quantity. Zero skips Add; excess existing quantity, negative/noninteger results
or uncertain matching stops. Do not remove items to repair the cart.

If adjustment is needed, the quantity chooser receives the exact remaining
result and original control/options. Unsupported values stop before selection.
Select and inspect the observed product-owned Add control, invoke once, and
observe after either a successful or uncertain return. The after-observation
must support the intended quantity and preservation of other contents before
the child succeeds. Comparing that child's before/after cart includes earlier
completed items without requiring a new generic mutable accumulator.

This fresh-cart entry may cost more calls than reusing the previous child's last
observation. Record the difference explicitly. Do not introduce mutable foreach
carry state merely to promise a lower call count; consider it separately only
if measurements justify it and its recovery contract is reviewed.

### Final reconciliation

Each child checks one requested item against the same saved final cart evidence.
Models choose semantic matches and price basis; calculator calls perform exact
arithmetic. Do not ask one small model to extract a JSON array of all cart facts.
Use actual observed quantities, not requested quantities substituted into a
wrong cart. Then total every requested line and assess the complete basket.

Reusing the whole final cart prose per item can increase aggregate prefill with
N. Keep that cost visible. Narrow original views are allowed only when perception
supplies the structure; a guessed parser or a model-written fact table is not an
optimization. Missing or oversized evidence stops without silent truncation.

## 3. What belongs in a model's context

| Task | Necessary context |
| --- | --- |
| Search preparation | Original item request, requested quantity and applicable global preferences. |
| Heading advice | Same request, saved query, current original headings and capture context. |
| Exact membership | Current original identities/decisive attributes and advice; all original records remain bound to the tool. |
| Product suitability | Current request, original scoped offer, actual location and relevant prior rejection. |
| Remaining quantity | Bound requested quantity and the original current cart evidence for matching. |
| Add readiness | Accepted/fresh offer, exact remaining result, current original cart and compact approved basket; detect changed existing/earlier lines before another Add. |
| Basket preflight | All original requests, compact original offer identities, exact line results and global constraints. |
| Final assessment | Complete requested coverage and exact totals, original final/baseline cart evidence and any unresolved facts. |

Do not inject `{{ user_input }}` indiscriminately in item tasks: it still denotes
the full original input. Use explicit child inputs. The bounded basket view includes the original accepted `page.url` and
`page.content` alongside requests and calculation operands, with a 24 KiB
whole-basket cap. Item suitability is checked in its child; the parent uses this
original prose for cross-item price basis, currency and allocation checks.
Oversize stops before Add rather than truncating facts or inventing a structured
semantic extractor. Full original accepted pages remain retained separately.

Authored prompts describe the current business question and available exits.
Combine a decision with its ordinary operation using existing `tool-or-route`
where the evidence and uncertainty boundary are the same. Preserve membership,
inspection, action and subsequent observation when they have different inputs
or failure meanings. Generic does not mean a multi-action shopping conversation.

## 4. Minimal capability changes and existing limitations

| Area | Baseline behavior | Implemented extension |
| --- | --- | --- |
| Input materialization | Bridge accepts declared string fields; root variables do not expose an item array automatically. | One bounded coder-invoked prepared-worklist adapter. Keep existing text UI/FFI transport. |
| Repetition | Snapshot foreach and explicit child inputs already exist. | Reuse them; keep all three iterations at root. No new loop node or expression language. |
| Child return | The child frame's saved node-output map enters the collected result. | Optional explicit child exports selecting named saved values through static bindings; legacy plans keep their current return shape. |
| Parent review view | All named foreach collections received the same full result array. | Optional bounded structural collection view using the existing field-mask semantics. Keep a separate complete accepted-results binding. |
| Totaling | `local.calculate` accepts two decimal operands. | One generic coder-invoked collection-total operation with strict coverage/outcome validation. |
| Observation | Live screen/service repeatedly scanned journals. | Separate bounded live projection; retain strict disk validation and recovery APIs. See [performance](PERFORMANCE.md). |

### Worklist and result contracts

The materializer produces a list of `{id, value}` records and preserved global
context. `value` is the upstream-prepared item object. IDs identify request
occurrences, not browser nodes or proof of distinct products. After preparation,
foreach snapshots and child inputs carry the original record without model
transcription.

Pin the context shape and validate request/quantity fields inside `value` in the
flow's result schema. Do not accidentally validate a flattened shape that the
generic tool does not produce.

Child `exports` are a finite map of names to existing dotted variable
paths: original request, accepted page, compact original identity, calculation
and relevant advice. No defaults, expressions, renaming programs or conditional
execution are hidden in bindings. Missing required exports fail the child return.
Pin and validate every subflow in the bundle. Resolve output differences between
initial/alternate paths at an explicit accepted-evidence join.

Keep immutable evidence before reducing child outputs. Current gateway logical
files such as `browser-text.json` can be overwritten by later calls; a filename
or recorded hash alone is not a retained historical payload. Preserve the
required original accepted/rejected values, and archive child evidence once in
bounded app-private storage before discarding full child output maps. Projected
collections must not leave dangling references to changing `latest` files.
The archive and export caps are specified in [G2-CONTRACT.md](G2-CONTRACT.md). Do not make a generic artifact database or query language.

Collection views only select existing structural fields, including through
arrays. They neither rank products nor parse cart prose. Their maxBytes applies
without truncation. A basket prompt should use the compact view rather than
accidentally serializing all child captures and raw query arrays.

### Exact collection totals

The `local.collection_total` tool receives the original expected worklist and the
collected result records through bindings. Independent authored static structural
pointers identify expected ID versus collected ID, outcome and exact decimal
amount; the original and collected record shapes need not match. The coder invokes the
operation with no model-owned collection or numeric operands.

Before returning a sum, require exactly one successful numeric result per
expected ID: reject duplicate/missing IDs on both input sides, extra actual IDs,
failed/skipped entries,
missing/null/invalid amounts and overflow. Use the decimal rules of
`local.calculate`; do not silently skip or round terms. Return count, exact
sum and the included original ID/amount terms. A zero total is legitimate only
if supported by complete valid terms, not an empty or failed result set.

The same operation serves planned and final totals, and can total non-shopping
records. It does not infer currency or price basis. Bind its exact sum into the
existing calculator comparison with the supplied budget. The reasoner/coder
still assess semantic facts and the authored strict/inclusive budget rule.
Arithmetic, accepted routes and membership are not independent proof of truth.

### Foreach qualifications that must remain explicit

- A child `end` outcome is collected; it does not automatically abort the root
  even when named `failed`. Unresolved/error exits during the mutation pass must
  use an actual failing transition. Do not continue adding later items after an
  unresolved child. Collection-total validation also rejects failed outcomes.
- `indexBy` adds a field to each collected entry; it is not a dictionary or join.
  Existing foreach does not validate unique/nonmissing keys. The prepared
  worklist contract must establish those properties.
- `between.inputs` is accepted but not applied by current execution. This plan
  does not rely on it; repair it separately before any future consumer uses it.
- Nested foreach outputs are not automatically in a child's returned
  `node_outputs`. Keep the loops at root and test any new export behavior.
- Foreach cursors are keyed by node ID. Use distinct IDs across loaded plans;
  do not assume invocation-scoped nested cursor names exist.
- `maxItems` resets its dispatch window on Resume. It is not a lifetime item
  limit; the input list bound supplies that limit.

## 5. Business and recovery limits

Initially support unambiguously distinct requested products. A duplicate or
overlapping request, or two offers that might satisfy the same request, stops
unresolved unless allocation is established. Opaque request IDs alone do not
solve semantic overlap. Do not silently aggregate duplicate quantities.

Keep merchandise-only, same-currency budgeting; exclude tax, shipping and
unrelated existing items. No checkout, order placement, subscriptions, coupon
optimization, FX, cheapest-combination search or removal. Those are additional
business procedures, not implied by increasing N.

An uncertain Add is observed, never replayed. Unresolved observation stops the
entire mutation pass and preserves the possibility of partial completion.
Cancellation does not roll back the cart. Crash recovery currently fails closed
for dangling intents or completed writes beyond the committed snapshot;
`onUncertain: refetch-state` is not implemented reconciliation. Preserve this
behavior. A new explicitly started run begins from the actual cart rather than
assuming an empty cart or replaying an old item cursor.

Current step/token/artifact caps are frame-scoped; parent deadlines remain
active. Root step limits do not cap all descendants. Bound list length, child
work and exported/archived evidence. The implemented aggregate envelope and
scripted measurements are recorded in [G2-CONTRACT.md](G2-CONTRACT.md). Do not relabel a per-frame 64 MiB allowance as a run-wide
64 MiB guarantee. Reject a design whose measured memory/disk growth is unsuitable
for the target phone rather than raising caps to conceal it.

## 6. Compatibility and delivery

New tools require an APK containing them. Export/collection-view fields require
updated schema, reader, validator, runtime, bundle admission and flow-review
consumer support. Old bundles retain existing behavior; older APKs must reject
unsupported fields clearly. Do not silently reinterpret existing foreach output
shape or resume snapshots. The compatible optional fields retain the v1 identifier; older readers reject
them as unknown fields. This work first pinned product version `0.1.0-rc.8` and
Android update code 43; native APK delivery was pending at that historical
checkpoint. Current release identity is maintained in [versioning](../VERSIONING.md).

Generic capabilities, tests and contracts belong on main and then in the Amazon
branch. The shopping procedure, prepared fixture and phone handoff belong on the
Amazon branch. Use isolated commits rather than importing all experimental
runtime changes from that branch without review.
