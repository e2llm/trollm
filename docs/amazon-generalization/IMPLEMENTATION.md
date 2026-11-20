# Count-independent execution and checks

This page describes the implemented contracts and historical source checks for
flow105 / `0.1.0-rc.8` / Android code 43, recorded on 2026-10-25. Current APK
downloads and release validation are linked from [versioning](../VERSIONING.md)
and [Android installation](../INSTALL-ANDROID.md). The later
[v342 examples](../../examples/run-342/README.md) retain their own flow, model
and APK identities.

## Implemented capabilities

- Bounded prepared worklists preserve each original request. Exact collection
  tools reject incomplete, duplicate or failed totals and invalid decimal input.
  Runtime argument bindings prevent the model from replacing saved collections.
- Reusable child tasks return bounded records and optional compact parent views.
  Original child outputs are retained separately; projection does not replace
  accepted evidence. Legacy return shapes and persistence boundaries remain
  covered by the [child-return contract](G2-CONTRACT.md).
- The count-independent procedure uses repeated discovery, execution and
  reconciliation steps. Fresh cart observations precede mutations; unresolved
  child outcomes stop the run, and uncertain writes are not automatically replayed.
  Shopping-specific behavior remains in the flow.
- Numeric model/tool phases, attempt/call correlation and persistence/observer
  measurements support offline summaries. Missing measurements remain distinct
  from zero. See the [measurement contract](PERFORMANCE.md).
- A bounded live observation projection feeds the screen and foreground service.
  Strict disk APIs retain corruption and recovery checks. Cached collection page
  reads still validate the full collection; no cache-page index or new cache-trust
  contract is implied.

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

## Historical implementation checkpoint

The 2026-10-25 source checks used scripted models and browser observations with
the production executor and local tools. One unchanged composition fixture
covered 1, 2, 3 and 8 items. The Amazon package checks covered unchanged bundle
bytes across 1/2/3/8 requests, original evidence in deciding prompts, baseline
and earlier-line price conflicts, uncertain Add, exact budget boundaries and
oversized basket rejection.

The recorded checks also covered child return shapes, persistence boundaries,
immutable child-output archives, bounded live observation and offline numeric
summaries. Host Rust, bridge stub, schema/bundle, Python, Android helper JVM and
viewer checks had passed at that checkpoint. These historical checks do not
identify a current APK or establish real-model product selection and physical
phone performance. Use the release records and published run evidence for their
respective validation scopes.

## Resource and acceptance limits

The 8-item limit is administrative. Each child retains a 256-step/1,474,560-token
frame cap and the root's 1,800-second deadline stays active. Three passes can
create at most 24 item frames, so root step/token caps alone do not imply a
whole-run cap. Exported records cap at 256 KiB each, collected arrays at 2 MiB,
and retained original child output archives at 64 MiB total. These are safety
limits, not target usage or a promised eight-item phone completion time.

The demonstrator assumes prepared input. Consumer shopping support, arbitrary
free-text intake, automatic crash reconciliation, allocation/optimization,
additional shopping sites and background cloud services are outside its scope.
A successful scripted run does not establish that a real model selected the
correct products or operands.
