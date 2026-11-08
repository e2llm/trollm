# Reliable repeated tasks and original evidence

These contracts support authored workflows across browsers, tickets and other
record-oriented tasks. They add no site rules, prose parser or expression
language. Models continue to operate through the existing tool-call boundary.
Phone acceptance of a particular flow remains separate from host tests.

## Iteration belongs to an invocation

Each execution frame owns its `foreach` cursors. Calling the same child flow for
another parent item creates a fresh frame and fresh enumeration. Nested plans
may use the same node name without sharing progress. Pause and cold Resume keep
the active frame's original snapshot, index, phase and collected results.

An explicit graph transition back to a `foreach` starts a new enumeration from
its current `items` binding. It clears the previous collection when entering;
an empty new list cannot expose an older successful collection. A pause without
`resumeAt` retains the current enumeration. A pause with an explicit `resumeAt`
enters that target afresh, just as a graph edge does.

Snapshots now store cursors inside frames. The runtime migrates old flat cursor
snapshots when an active owner is unambiguous, drops completed child cursors whose
frames no longer exist, and refuses ambiguous or mixed representations. Item
snapshots use content-addressed names, so another invocation cannot overwrite
different active enumeration data. Do not downgrade an APK to resume a run
written by a newer runtime.

## Leave a collection on a declared child outcome

An optional `foreach.onItemOutcome` map chooses an authored transition after a
body reaches one of its declared `end.outcome` values:

```yaml
onItemOutcome:
  accepted: use_candidate
  uncertain: {status: failed, reason: A write needs independent reconciliation}
```

The body must declare these end outcomes. Keys are bounded identifiers, not
words extracted from a model answer. The runtime first records the child result,
advances the cursor and publishes the collected prefix through the normal output
contracts. It then takes the selected edge, skipping `between` and remaining
items. Invalid exports or oversized collection views still fail before the exit.
A paused outcome exit requires `resumeAt`, so Resume does not replay a completed
selection. Actual execution failures and uncertain mutation journals are not
converted into child outcomes by this map.

If the map is omitted or the outcome has no entry, the usual collection behavior
continues. In particular, an end named `failed` remains an ordinary collected
child outcome unless explicitly routed; a failing transition aborts the run.
Cursor ownership applies independently of whether this optional map is present.

## Preserve the facts needed for the next decision

- Keep the original request and quantities alongside any shortened search
  wording. Search wording does not replace product, ticket or other constraints.
- Pass the intended operation and target entity together. “Find Add” is weaker
  than identifying which product the requested Add belongs to.
- Preserve attributes that distinguish candidates: labels, selectors, roles,
  relationship fields and relevant original text. A smaller view that removes
  these facts can be harder to interpret than a slightly larger one.
- When competing numbers distract the model, first identify the measurement's
  original record without showing numeric values. Read its value from the saved
  original, verify current ownership when acting, and use a calculator for exact
  comparison. This is a candidate technique, not mandatory decomposition.
- Once arithmetic determines a direction, ask which observed control implements
  that direction. Do not make the control-selection question redo the arithmetic.
- Keep reasoner advice in prose. The coder selects an original record through a
  tool or chooses a declared no-selection exit. Exact reasoner IDs and currency
  strings are not a replacement for that boundary.

Structural filtering and unique membership prove shape and membership, not
semantic suitability or freshness. Literal word matching is appropriate only
for a genuinely literal requirement. See [built-in tools](BUILTIN-TOOLS.md) and
the [model-specific prompting method](FLOW-PROMPTING.md).

## Recovery needs a bounded change of approach

Distinguish an unsupported target, a malformed unexecuted call, a missing read,
and a write whose effect is unknown. Observe after an uncertain write; do not
repeat it merely because an answer was lost. Give recovery a bounded next
observation or alternative, then an honest stop. Replacing every failure with a
back edge can repeat an unchanged problem until a large global budget expires.

Count model calls as well as tokens and elapsed time. Many tasks that invoke one
fully bound operation each can dominate a run. Compare coherent coder tasks
against additional microsteps, retaining verification before writes. Shared
templates remove duplicated source, not the cost of executing those templates.

## Replay one observation before repeating the mission

`scripts/android/make_selection_replay.py` builds an ordinary installable probe
from a JSON array of saved original records. It grants only local read tools and
`flow.choose`; there are no browser tools or external effects.

```sh
python3 scripts/android/make_selection_replay.py \
  --records /private/records.json --question /private/question.txt \
  --output /private/selection-replay.tar
```

Add `--advice /private/advice.txt` to freeze ordinary adviser text and isolate the
coder's handoff. An optional `--fields` JSON artifact mask changes only the model
view; the original objects remain the selection source. Keep expected IDs and
outcomes outside these files and outside the bundle. Synthetic unit fixtures
are labelled as such; use reviewed real captures for model-quality experiments.

Use the intended model/settings and record their actual identity. Both the
`selected` and `declined` terminal nodes mean the probe executed; inspect its
chosen route and selected artifact to score correctness. A successful status
alone is not a correct answer. Include valid choices, no suitable choice,
ambiguity, distracting numbers and changed record order; count unnecessary
refusals separately. Keep raw captures and generated bundles private by default.

The helper tests validate packaging and grants. Scripted Rust tests validate
binding, selection, cursor and recovery behavior. Neither proves that a real
model interprets the question correctly or completes the larger workflow.
