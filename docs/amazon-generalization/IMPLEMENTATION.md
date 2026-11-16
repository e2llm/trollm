# Implementation sequence and checks

Historical implementation record for flow105 / `0.1.0-rc.8` / Android code 43
(2026-10-25). Current release identity, CI evidence and APK downloads are linked
from [versioning](../VERSIONING.md) and [Android installation](../INSTALL-ANDROID.md).
The work packages below retain the approved implementation sequence and
acceptance criteria; the [historical checkpoint](#historical-implementation-checkpoint)
records that stage's status. Scope and implemented contracts: [README](README.md), [design](DESIGN.md),
[performance](PERFORMANCE.md). User input is already prepared for the first
step; consumer input UX and natural-language normalization are excluded.

Main agent owns design and integration. Sol/medium agents can implement bounded
packages with distinct ownership after the contract decisions. Keep one local
Rust build owner, reuse caches, cap concurrency and check disk/RAM first. No
model downloads, SDK installations or phone/browser runs are needed for these
source tasks. The phone test owner owns the eventual physical-phone check.

## Approved work packages

### G0 — Prove the composition and settle the narrow contracts

Start with the actual foreach executor and existing ticket-triage fixtures.
Build a tiny scripted slice: prepared worklist → reusable discovery child →
collected results → bounded total → one mutation child → observation. Use the
same plan with one, two and three items. This is an implementation spike with
host tests, not a phone experiment or another shopping prompt trial.

Confirm child outcome behavior, result shape, original request propagation,
artifact retention, parent compact view, cursor/Resume handling and per-frame
limits. Count current snapshot/collection bytes at each boundary. Decide the
exact worklist/total API and optional export/view syntax. Do not implement from
new field names without reviewing schema/reader changes together. This review
settled the implemented syntax in [G2-CONTRACT.md](G2-CONTRACT.md).

Exit: one reviewed contract sketch, representative passing host fixtures, and a
finite memory/disk/work envelope. If projection would discard historical
evidence or a child failure could allow the next Add, resolve that before porting.

### G1 — Prepared input and exact collection tools

Implement only the bound worklist adapter and strict collection total in
`trollm-mcp`, with schemas, grants, dispatch validation and docs. Preserve the
existing text input transport. Domain-specific quantity/request requirements
stay in flow output validation, not product-specific tool code.

Test malformed/oversized input, empty lists, 1/2/3/max/max+1 items, stable unique
IDs, unchanged original prose, missing/extra/duplicate result IDs, failed/skipped
outcomes, invalid decimal values, exact sums and overflow. Test bound arguments
cannot be replaced by model-provided collections. Add at least one non-shopping
fixture to demonstrate the operations' generic contracts.

Exit: independently reviewed generic code on main; schema/docs/helper consumers
aligned. No new browser procedure yet. Depends on G0.

### G2 — Bounded child returns and parent views

Implement the reviewed optional export and collection-view behavior, retaining
legacy output/Resume compatibility. Preserve original child evidence before
dropping large node-output maps; views are projections, not replacements for
the complete accepted record. Reuse existing field-mask validation and static
path binding rather than inventing an expression evaluator.

Test missing exports, wrong shapes, oversized views, initial/alternate joins,
old bundles/snapshots, child return across each persistence boundary, and
repeated item/node IDs scoped as documented. Verify no current logical artifact
can overwrite the evidence referenced by a previous item. Measure parent-state
growth and retention with large synthetic observations.

Exit: generic capability and focused tests on main, with flow-review/package
consumers updated. If G0 proves an existing binding can do a part, reuse it and
remove that proposed extension. Do not add optional machinery without a consumer.

### G3 — Count-independent Amazon package

Port the reviewed generic capabilities into `e2llm-amazon-dev-1`. Replace ordinal
chains with root discovery/execution/reconciliation foreach nodes and reusable
children. Add the bounded prepared fixture and compact basket context. Reuse
existing original-evidence, quantity, membership, input-proof and uncertain-write
procedures. Adapt initial/alternate acceptance to one exported item contract.

Use a count-neutral package identity such as `amazon-cart`; do not describe the
new package as dual-item. Update bundle generation/install metadata together.
Retain the pinned flow104 commit/artifact for comparison without overwriting old
run identities or maintaining a permanent second copy of the new procedure.

Replace first/second line totals and reports with complete collected totals.
Ensure preflight precedes every Add. Start every mutation child from fresh cart
state; unresolved child exits fail the whole run. Show partial completion
honestly. Reinspect changing facts without replaying writes or implying atomicity.

Remove obsolete ordinal tasks/prompts after tests establish replacement paths.
Update the flow-review phase map so humans see one repeated procedure and its
recovery, rather than a separate phase per configured item. Measure actual normal
and recovery call counts; do not count fewer YAML definitions as a speedup.

Exit: same exact package hash runs 1/2/3-item host scenarios with different item
orders/quantities/budgets. No changes to product names, prices or item count in
the package between runs. Depends on G1/G2.

### M1 — Complete the measurement record

Implement the missing per-attempt/call correlation and numeric boundaries in
[PERFORMANCE.md](PERFORMANCE.md). Include original/compact response sizes,
artifact/view sizes, actual prompt bytes/tokens, model phases, tool elapsed time,
step time and persistence/observer costs. Keep success, partial failure and
unknown measurements distinct. Add an offline summary that reads each journal
once and does not require a new telemetry service.

Exit: synthetic fixtures demonstrate correlation across repeated calls and
worker epochs, timing containment, no double-counting and null/zero semantics.
No raw content or private IDs enter the shareable aggregate report. This can
proceed in parallel with G1/G2 after the field definitions are reviewed.

### M2 — Cheap live observation

Add the bounded worker-owned progress projection and UI/service consumer path.
Seed it by strict reconstruction when necessary; do not replace the existing
strict disk APIs with an unchecked offset cache. Bound event batches, Android
technical-line retention and inactive-run projection caches.

Exit: idle active-run polling parses no old journal prefix, appended records are
processed once per update, memory remains bounded, and cold reconstruction
matches the live view. Preserve corruption, cancellation and uncertain-mutation
tests. This optimization must not change workflow decisions or journal authority.

### M3 — Optimize measured cache/artifact work selectively

Measure full cached-collection scans per selected page, repeated exact-value
size counts, artifact rereads and whole-snapshot serialization. Reuse validated
bytes/counts only within an identified unchanged generation. Indexing cached
pages requires a reviewed integrity/invalidation contract; do not trade away
validation merely for a lower benchmark.

Exit: measured work/bytes scale as intended with identical results and failure
behavior. Defer any optimization whose cost is insignificant or whose safe
generation contract is unresolved. No sampler/backend/thread tuning by guesswork.

### G4 — Integrate, document and hand off

Run focused workspace/bridge, schema/bundle and relevant Android JVM/helper
checks, then source/dependency HIGH/CRITICAL CI gates and a native APK build.
Update capability docs, authoring guidance, onboarding links, package presentation
and the branch's handoff. Ensure curated export contains portable design/docs
and no private logs, operator history, reviews or credentials.

Record the exact source/package/build identity and test scope. Leave long CI
jobs running without polling loops. The later phone test uses the same
generic package with the prepared two-item input and original acceptance goal;
it does not use a special two-item graph.

## Regression matrix

| Concern | Required scripted/integration cases |
| --- | --- |
| Genericity | N=1,2,3,max and rejection of 0/max+1; reordered and renamed IDs; varied product prose, quantities, currency context and budgets; identical bundle bytes. |
| Coverage | Missing/extra/duplicate IDs, skipped/failed child, invalid exported result; no Add on incomplete preflight. |
| Existing cart | Zero/partial/full/excess requested quantity, unrelated lines, semantic overlap unresolved, original baseline retained. |
| Prices | Unit versus quantity-inclusive total, mixed/unknown currency, conditional discounts, baseline requested-line price differs from offer, earlier committed line changes before item k, changed offer, strict equality failure and inclusive equality success. |
| Discovery | Supported initial/alternate prospect, rejected alternate stops, original rejection retained, fresh IDs and accepted page used downstream. |
| Quantity/action | Unsupported/disabled option, wrong form ownership, no-op for already satisfied quantity, one Add attempt followed by observation. |
| Partial completion | Failure before first Add; uncertain Add at item k; previously completed items retained; no action for k+1; cancel/explicit Resume/new-run semantics. |
| Final check | Every requested identity/quantity, missing/extra quantity, full-list exact sum, unrelated contents preserved; never infer success from tool return alone. |
| Scale | Large bounded original observations, collected-state bytes, prompt tokens and executed-call growth; observer/cache read counts and bounded retention. |

Scripted tests prove wiring and safety properties of scripted choices. They do
not prove that a real model chooses the correct product or operands. Later phone
comparisons must preserve model/settings and comparable observations, report
semantic correctness as well as time, and distinguish cold loading from warm
generation. A faster wrong choice is not an improvement.

## Original effort estimates

These were planning estimates from the initial inspected gaps, not measured
time spent, elapsed CI time or a delivery promise. Evidence retention and child
return contracts were the largest initial uncertainty; G0/G2 have since settled
those contracts. The excluded input-UX project remains outside scope.

| Work | Focused hours, approximate | Main uncertainty |
| --- | ---: | --- |
| G0 composition/contracts | 3–6 | Return/projection and evidence retention mechanics. |
| G1 two local tools | 4–8 | Strict numeric/coverage contracts and fixture plumbing. |
| G2 returns/views | 6–12 | Snapshot compatibility and original-evidence retention. |
| G3 generic flow | 8–16 | Porting fresh-session/recovery bindings without ordinal assumptions. |
| M1 measurement | 4–8 | Correlation and failure-path phase accounting. |
| M2 live observation | 6–12 | Observer ownership, reconstruction and strict API separation. |
| M3 selective I/O optimization | 3–8, optional | Whether a safe cache index is warranted by measurements. |
| G4 checks/docs/integration | 4–8 | Cross-branch capability baseline and CI failures. |

Mandatory estimate: 35–70 focused hours; M3 adds 3–8 if justified. Agent work can
overlap G1/G2 with M1, then G3 with M2. Integration/review remains coordinated.
Physical acceptance and its possible follow-up corrections are outside that
estimate. This historical estimate is retained for context; the next delivery
steps are final host checks, CI/security gates and the native APK, followed by
separate physical acceptance.

## Completion boundary

Source implementation is complete when count-independent execution, required
metrics and bounded live observation pass their checks; docs and exact CI/APK
handoff are ready. Demonstrator acceptance additionally needs a real-model phone
run meeting the prepared mission. Consumer-grade shopping support, arbitrary
free-text intake, automatic crash reconciliation, allocation/optimization,
additional shopping sites and background cloud services are not implied.

## Historical implementation checkpoint

This checkpoint records the 2026-10-25 source work. Its pending delivery checks
are historical, not the status of the current release. Later phone evidence is
published with its exact identities in [examples/run-342](../../examples/run-342/README.md).

- G0/G1: one unchanged host fixture runs 1, 2, 3 and 8 items with the production
  worklist, calculator and complete-collection total tools. Prepared values are
  bound; replacement model operands are rejected before mutations. Invalid
  counts and incomplete/duplicate/failed totals are covered.
- G2: optional exports, bounded full/compact collections and immutable child
  output archives are implemented with legacy-shape and persistence-boundary
  tests. See [exact caps and retention](G2-CONTRACT.md).
- M2: a separate bounded live projection feeds the screen and foreground
  service. Strict disk APIs retain their prior corruption/recovery behavior.
- G3: the flow105 `amazon-cart` package is implemented. The real-package
  `amazon_generalized` host suite passed 8 tests, including unchanged bundle
  bytes for 1/2/3/8 requests, original evidence in deciding prompts, baseline and
  earlier-line price conflicts, uncertain Add, exact budget boundaries and oversized basket rejection.
  The generic `count_independent` composition suite passed 2 tests. These use
  scripted models/browser observations and production executor/local tools.
- M1: numeric model/tool phases, worker-epoch/attempt/call correlation and
  persistence/observer measurements are implemented, with an offline aggregate
  reader. Workspace unit/integration checks, stub bridge tests, Clippy, formatting,
  schema/bundle and Python checks pass. Android helper JVM tests and the
  flow-review TypeScript/tests also pass; native/Compose compilation stays in CI.
- G4: documentation, schema/bundle and host checks are complete. A single
  Astra/max adversarial review was completed; timing documentation and the
  technical-event rendering findings were corrected and regression checked.
  CI/security gate results, a native APK identity/checksum and physical-phone
  acceptance were still pending at this checkpoint.
- M3: a cache-page index is deferred. Full-collection validation still occurs
  for each selected page; changing that trust/invalidation contract is not
  needed for the bounded live observer fix. New measurements expose the cost.

The 8-item limit is administrative. Each child retains a 256-step/1,474,560-token
frame cap and the root's 1,800-second deadline stays active. Three passes can
create at most 24 item frames, so root step/token caps alone do not imply a
whole-run cap. Exported records cap at 256 KiB each, collected arrays at 2 MiB,
and retained original child output archives at 64 MiB total. These are safety
limits, not target usage or a promised eight-item phone completion time.
