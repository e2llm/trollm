# Keep separate results from the same read tool

`resultArtifact` is an implemented optional task-registry field. It lets an
author keep a broad observation and a narrower follow-up under different names,
even when both use the same remote read method. The gateway saves the native
result directly; the model does not copy it or choose its storage name.

For example, a later decision may need both a document and its selected section.
Without separate destinations, the second read can replace the tool's usual
result file. A task-specific destination preserves both through the existing
artifact/output contracts, without adding a tool or model call.

## Authoring example

These fragments illustrate storage wiring, not a complete executable flow. Supply
the service connection, actual argument schemas/bindings, prompts and graph for
your tool. The root plan must already declare its result storage policy:

```yaml
mcp:
  policies:
    service.read:
      class: read
      resultArtifact: read-default
```

Two registry tasks can choose different destinations:

```yaml
tasks:
  read-document:
    model: coder
    prompt: prompts/read-document.md
    tools: [service.read]
    completion: tool-result
    loop: {maxSteps: 1}
    resultArtifact: document-original
    outputContract:
      outputs:
        document: {fromArtifact: document-original, format: json, type: object}
      requireArtifactComplete: [document-original]
  read-section:
    model: coder
    prompt: prompts/read-section.md
    tools: [service.read]
    completion: tool-result
    loop: {maxSteps: 1}
    resultArtifact: section-original
    outputContract:
      outputs:
        section: {fromArtifact: section-original, format: json, type: object}
      requireArtifactComplete: [section-original]
```

The coder still invokes the granted tool. Its arguments determine what is read;
`resultArtifact` only determines where the existing gateway saves that result.
The normal result validation and shaping policy still apply. Bind the saved
outputs into later tasks rather than asking the model to reproduce them.

## Restrictions and replacement behavior

- Use exactly one remote read tool, `completion: tool-result` and one generation
  (`loop.maxSteps: 1`, or omit the loop). Local tools, control tools, mutations,
  pagination and `serveFrom` are excluded. Actual gateway classification must
  also be read; a registry declaration cannot downgrade it.
- The task must declare a matching artifact output and include its name in
  `requireArtifactComplete`. Node output overrides must preserve the match.
- Names contain 1–64 ASCII letters/digits, `_` or `-`, starting with a letter or
  digit. Native proof names and `model-failure-` names are reserved; the
  [field schema](../schemas/task-result-artifact.v1.schema.json) lists them.
  A name cannot collide with another tool's destination. Producers using the
  same tool can deliberately share one name across authored alternatives.
- Names are run-wide static destinations, not automatic per-iteration archives.
  Revisiting a producer replaces that destination. Use separate destinations or
  retained task outputs where later work needs an earlier result.
- Before generation, core clears that producer's previous outputs and marks its
  destination incomplete. If that invalidation fails, the run stops without an
  authored retry/error transition: an old complete file may still exist.
  Ordinary success requires the named artifact from the current call, not merely
  an older file on disk. Unsupported gateway adapters reject the capability.
- Live and cache paths use the existing artifact validation, hashing and
  completeness machinery. Cache hits remain cache hits: a different name or
  publication timestamp does not prove a new capture or fresh remote state.

The field changes no wire argument, credential, cache key, deadline, tool
classification or model role. It does not interpret prose, prove that a selected
source is relevant, or make partial provider data complete. Keep source/session,
completeness and semantic checks in the authored procedure. Existing mutation
recovery rules still apply.

## Compatibility and validation

Omitting the field preserves existing behavior. Older readers reject this unknown
task field; importing a package is not a runtime upgrade. Android needs an APK
containing the capability. Source tests do not establish device acceptance.

The additive JSON Schema checks local shape. Rust additionally checks the root
policy, actual gateway class, output declarations and destination ownership.
The [architecture](ARCHITECTURE-ANDROID.md#task-specific-result-artifact-names)
defines the runtime boundary. Focused checks are:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p trollm-core -p trollm-mcp --test task_result_artifact --no-default-features
CARGO_BUILD_JOBS=2 cargo test --locked -p trollm-uniffi --features stub controlled_gateway_forwards_task_result_artifact
python3 scripts/test_task_result_artifact_schema.py
python3 scripts/validate-examples.py
```

These use scripted models, storage fixtures and a local mock MCP service. They
cover distinct original/narrow destinations, unchanged wire arguments, live/cache
provenance, invalid declarations, current-call enforcement, failed replacement,
unsupported adapters, warm/cold recovery and a metadata-write failure that must
not reach a stale-data join. They do not test live-model selection accuracy.
