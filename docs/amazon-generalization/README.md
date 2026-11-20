# Generalizing the Amazon demonstrator

These design and implementation records began with flow105 / `0.1.0-rc.8` /
Android code 43 on 2026-10-25, compared with the earlier flow104 / Android code 42
procedure. See [the historical checkpoint](IMPLEMENTATION.md#historical-implementation-checkpoint).
Current release identity and APK downloads are in [versioning](../VERSIONING.md)
and [Android installation](../INSTALL-ANDROID.md). The later
[v342 phone examples](../../examples/run-342/README.md) retain their own flow,
APK and model identities; they do not establish general workflow reliability.

This is a **technological demonstrator** of trollm executing complex authored
procedures, not a complete shopping product.
Assume the input is already prepared usefully for the first step. Input forms,
general natural-language request normalization and consumer onboarding are out
of scope. The same mechanisms should support other procedures such as ticket
triage or hotel comparison without teaching the executor their business rules.

## Target

One package handles a bounded list of distinct requested products. Two items
are an acceptance input, not a graph shape. Changing products, quantities,
budget, order or supported item count must not require regenerating YAML,
changing prompts or rebuilding the package. The initial list bound is
eight items; it is a resource guard, not a claim that eight-item phone runs
already fit the acceptance deadline.

```mermaid
flowchart LR
    A[Prepared request] --> B[Materialize bounded worklist]
    B --> C[Observe baseline cart]
    C --> D[Discover and assess each item]
    D --> E[Check complete basket and budget]
    E --> F[For each item: fresh facts, Add if needed, observe]
    F --> G[One final cart observation]
    G --> H[Verify each requested line]
    H --> I[Exact total and final assessment]
    I --> J[Report]
    D --> U[Stop unresolved]
    E --> U
    F --> U
    H --> U
    I --> U
```

The three iterations are ordinary `foreach` passes. Per-item model tasks receive
the same stable instructions and the current original request. The root handles
cross-item concerns. No model-generated JSON worklist, prose routing words,
expression language, Amazon executor branch or automatically repeated Add is
introduced.

## Why generalization may help

The flow104 baseline often supplied the whole mission and required a model to
recover which product FIRST or SECOND meant. The item-local procedure removes that repeated
selection problem and makes fixes apply equally to every item. Keeping the
original request also prevents a shortened search phrase from silently losing
constraints. Authored generic prompts remain necessary; product facts and
ordinal-specific procedure are the parts to remove.

This is a testable hypothesis, not a performance result. Shared YAML alone does
not remove model calls or prefill. The plan explicitly measures executed calls,
input tokens, model phases, tool work and observer/storage overhead. It retains
fresh observation and uncertain-write boundaries even when they cost calls.

## Reading order

1. [DESIGN.md](DESIGN.md): procedure, data handoffs, implemented capability
   changes, existing limitations and safe stopping behavior.
2. [PERFORMANCE.md](PERFORMANCE.md): verified size/timing instrumentation,
   repeated scans, measurement boundaries and implemented bounded live observation.
3. [IMPLEMENTATION.md](IMPLEMENTATION.md): implemented capabilities, regression
   coverage, resource limits and the scope of historical checks.

Use [versioning](../VERSIONING.md) and the linked release records for current
APK delivery and validation. The published [v342 examples](../../examples/run-342/README.md)
describe their exact historical flow, models and APK; they do not establish
acceptance of other builds or inputs.
