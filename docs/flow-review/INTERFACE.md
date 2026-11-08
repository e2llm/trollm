# Flow review interfaces

Approved implementation contract, 2026-10-24. Main agent owns changes to this
document; implementers report any necessary material deviation before coding it.

## Authored presentation

Optional package-relative `flow-review.json` (UTF-8 JSON):

```json
{
  "format": "trollm.flow-presentation.v1",
  "title": "Check a total",
  "purpose": "Check two amounts against the user's budget.",
  "inputsDescription": "Text containing two amounts and a separate budget.",
  "resultDescription": "A report with arithmetic evidence and model advice.",
  "phases": [
    {
      "id": "check",
      "title": "Check the amounts",
      "description": "Select operands, calculate and compare with the budget.",
      "nodes": [{"flow": "check-total", "node": "sum"}]
    }
  ]
}
```

Required: format, nonempty title/purpose, phases. Input/result descriptions are
optional. Phase id/title/description/nodes are required. IDs must be unique;
each reference is a `(flow ID, node ID)` pair and belongs to at most one phase.
Validate nonempty strings, bounds and references; report unknown/duplicate
references. Missing descriptions are not an execution error. Unassigned nodes
are listed explicitly and remain accessible. Do not infer business meaning from
task IDs or prompts. Supply a schema in the component and a checked example.
Invalid presentation produces review diagnostics without hiding the graph.
Execution validation and presentation validation have separate diagnostics.

## HTTP API v1

All responses JSON except browser assets. Semantic validation failures return
an inspection document with `valid: false`. Request failures use
`{ "error": "safe human-readable explanation" }`:

| Status | Meaning |
| --- | --- |
| 400 | The request or package cannot be safely read or inspected. |
| 413 | The request body exceeds the 64 MiB limit. |
| 408 | The request exceeded the 60-second response deadline. |
| 429 | Both inspection slots are occupied; retry after an earlier request finishes. |

Other client errors, such as an unsupported media type or malformed JSON body,
retain their HTTP status with the same safe JSON error envelope. A response
timeout does not cancel inspection work that is already running: its slot stays
occupied until that work exits. Requests beyond the two-slot bound are rejected
without joining a queue. No traceback, credentials or absolute host paths are
returned. Keep identifiers opaque where appropriate.

- `GET /healthz`: `{ "status": "ok" }`.
- `GET /api/packages`: `{ "packages": [{ "id": "opaque", "name": "name", "kind": "source" }] }`.
  Catalog may include `kind: "bundle"`. Empty mounted directory is valid.
- `GET /api/packages/{id}`: inspect mapped item. Optional `entryFlow` and
  `taskRegistry` query selections use safe package-relative paths.
- `POST /api/upload`: raw uncompressed tar bytes, Content-Type
  `application/octet-stream`, returns inspection document.
- `POST /api/inspect`: JSON `{ "files": [{ "path": "relative/path", "content": "UTF-8 text" }], "entryFlow": "optional.flow.yaml", "taskRegistry": "optional.tasks.yaml" }`.
  Used for folder/multiple-file imports and explicit root/registry selection.
  Preserve relative paths; reject duplicates and unsafe names.

Inspection document:

```json
{
  "format": "trollm.flow-review.v1",
  "packageId": "sha256 of sorted path/content identities",
  "name": "display name",
  "entryFlow": "example.flow.yaml",
  "taskRegistry": "example.tasks.yaml",
  "candidates": { "entryFlows": [], "taskRegistries": [] },
  "valid": true,
  "diagnostics": [{ "level": "warning", "code": "presentation-missing", "message": "No authored phase descriptions", "path": "flow-review.json" }],
  "files": [{ "path": "example.flow.yaml", "content": "...", "sha256": "...", "sizeBytes": 123 }],
  "plans": [{ "path": "example.flow.yaml", "id": "example", "version": 1, "entryStep": "start", "limits": {}, "onLimit": "abort", "models": {}, "mcp": {}, "nodes": {} }],
  "registry": { "path": "example.tasks.yaml", "id": "example", "version": 1, "tasks": {} },
  "setup": null,
  "presentation": null,
  "unassignedNodes": [{ "flow": "example", "node": "start" }]
}
```

`entryFlow`, `taskRegistry` and `registry` may be null for incomplete/ambiguous
sources. Plan/registry structures use current serialized camelCase contracts,
without absolute source/base directories or recursive duplicate subflow objects.
`plans` includes each loaded flow once. Diagnostic objects contain `level`,
`code`, `message` and `path`; node-specific context currently appears in the
message rather than a separate structured diagnostic field. `valid` concerns
execution/package validation; presentation problems use codes prefixed
`presentation-` and are reported separately. Invalid source is still returned
where safe to inspect.

Execution validation covers the selected root and its referenced subflows.
Other supplied flow sources remain visible with `plan-unselected` warnings;
their execution contracts have not been checked as additional entry points.

Identify a source package using its manifest when available. Otherwise choose
only an unambiguous root/registry (a unique root not referenced as a subflow is
acceptable); return candidates and a diagnostic when selection is required.
Manifest file hashes and setup pins must be checked rather than silently fixed.
Unused supplied files remain visible and included in package identity. Stable
identity must not depend on file order or mounted absolute path.

## Feedback and comparisons

The browser attaches comments to a package, phase, qualified node, registry task,
prompt path or field. Markdown export includes package identity, source target,
relevant current value/excerpt and the human's comment. It requests that the
authoring LLM update sources and presentation, run existing validation, and
describe changes. Clearly retain the original revision anchor after reloading.
Do not pretend unresolved comments were satisfied automatically.

Compare two inspection documents by qualified flow/node and task/file identity.
Summarize added/removed nodes, changed edges, grants, completion/recovery,
limits, setup and phase membership. Show ordinary text changes for prompts;
do not claim semantic equivalence. Renamed/unmatched nodes appear as removal
and addition rather than guessed matches. Unchanged files must not dominate
the review. Package bytes remain untouched by this interface.

Plan inspection preserves optional `exports` and per-collection `view` fields
through the typed core contract. These control which original saved values a
child returns and what a parent receives; they do not execute expressions.
See [child-return limits](../amazon-generalization/G2-CONTRACT.md).

Collection nodes also preserve optional `onItemOutcome`. The graph draws an edge
for each declared outcome, the details panel shows the map, and comparisons
include its changes. Terminal and paused targets retain their usual meaning;
an explicit `resumeAt` is part of the transition. These are authored child-end
outcomes, not words interpreted from model prose. See
[declared collection exits](../FLOW-RELIABILITY.md#leave-a-collection-on-a-declared-child-outcome).
