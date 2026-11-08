# Explicit child returns and retained evidence

Status: implemented in flow105 / `0.1.0-rc.8` / Android code43 source.
Host unit/integration checks pass; CI/native APK and phone acceptance are pending.

Implemented optional syntax:

```yaml
exports:
  request: {from: request}
  accepted: {from: accepted.outputs.original}
```

Omitting `exports` preserves the legacy complete node-output return shape.
Explicit exports contain 1–32 named bindings, with static dotted paths (including
unsigned array indices); missing bindings fail before child pop or cursor advance.
Exports are capped at 256 KiB before any archive write. The collected record is `{id, outcome, outputs: {request, accepted}, evidence}`.
Names and path segments are bounded ASCII names; paths are at most 256 bytes.

A collection may add `view: {fields: {...}, maxBytes: N}` using the existing
artifact field mask. It selects fields through arrays and rejects oversized views
without truncation. A new-view collector permits at most eight names; any collector using exports or
views caps the unprojected accumulated result array at 2 MiB before copying it.
Legacy collectors retain their existing bounds. A separate collection without a view keeps all exported
values. Collector outputs are saved in bound vars and, only in plans with explicit
`exports`, in `node_outputs`, so another child can export them through ordinary
bindings. Legacy nested child return maps keep their previous omission of
foreach nodes.

Before an explicitly exported child frame is discarded, its complete bound
`node_outputs` map is saved under `child-evidence/<sha256>.json`. Each collected
record's `evidence` carries that run-relative path, hash and byte count. An
existing content-addressed file is compared with the original bytes before reuse.
Symlinks and unexpected directory entries fail closed. Limits are 128 completed
exported child returns, 8 MiB per archive payload and 64 MiB in the run archive;
archive bytes also count against the child's existing artifact budget. Archives
survive mutable logical artifact-file overwrites and all normal return snapshots.

This retains bound values, not every remote artifact ever fetched. Authors must
bind full original accepted/rejected evidence before projecting it; an output
which was already viewed contains only that view. Merely exporting a mutable
filename does not preserve the referenced file's historical contents.

Foreach outcomes keep their legacy meaning when `onItemOutcome` is omitted:
an `end` named `failed` is collected, not a root abort. Optional declared outcome
exits can stop after publishing the collected prefix; actual failing transitions
still abort. Cursors now belong to each invocation frame, including nested
iterations and repeated child calls; no carry-state contract is added. See
[flow reliability](../FLOW-RELIABILITY.md) for migration and Resume semantics. `maxItems` remains a per-Resume dispatch window.
The immutable prepared worklist supplies the lifetime count bound.

The root deadline applies while children execute, but step/token/artifact budgets
remain frame-scoped. For a prepared list of at most eight, each pass permits at
most eight child frame budgets plus its root work; three distinct root passes
permit at most 24 child invocations. A root step cap is not a global descendant
step cap. The archive has its own run-wide 64 MiB cap.

## Scripted composition measurement

The `amazon_generalized` integration suite runs the unchanged packaged source
and tar identity for N=1,2,3,8 with scripted models/browser observations and the
production gateway/local arithmetic. On the short original observations in that
fixture, terminal parent snapshots and retained child evidence measured:

| Requested items | Parent snapshot bytes | Immutable child evidence bytes |
| --- | ---: | ---: |
| 1 | 103,420 | 34,923 |
| 2 | 116,983 | 68,363 |
| 3 | 130,666 | 104,292 |
| 8 | 208,293 | 286,112 |

These are fixture measurements, not phone memory or real-page bounds. A separate
case supplies three complete approximately 13 KiB original product observations;
the authored 24 KiB basket view fails before totaling or Add, without truncating
the retained originals. Maximum list length does not promise that every possible
bounded page combination will fit the basket view.

Executed calls from the same scripted runs (ordinary tools include local and
remote gateway dispatches; `flow.choose` is runtime control, not a gateway call):

| Scenario | Coder calls | Reasoner calls | Total model calls | Gateway calls |
| --- | ---: | ---: | ---: | ---: |
| 1 requested item, success | 118 | 10 | 128 | 102 |
| 2 requested items, success | 190 | 17 | 207 | 164 |
| 3 requested items, success | 274 | 25 | 299 | 236 |
| 8 requested items, success | 658 | 62 | 720 | 566 |
| 2 items, accepted alternate | 203 | 20 | 223 | 175 |
| 3 items, uncertain Add at item 2, then stop | 196 | 17 | 213 | 168 |

Fixtures vary requested quantities as well as count; these are executed paths,
not a fixed per-item multiplier. Shared definitions remove ordinal duplication,
not model work. In particular, 207 model calls for the two-item case are not a
latency improvement claim. Real-model prefill/generation and tool timings still
need a phone run. The fixture controls semantic choices; it proves no model
competence in price interpretation, currencies or product allocation.
