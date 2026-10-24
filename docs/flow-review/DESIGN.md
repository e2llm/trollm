# Flow review design

Approved scope: 2026-10-24. Delivery status: [implementation plan](IMPLEMENTATION.md).

## Purpose and workflow

Make large LLM-authored flow packages understandable enough for a human to give
useful feedback. One browser page presents purpose, inputs, expected results,
business phases, consequential operations, limits and recovery. Selecting a
phase or node reveals the executable graph, prompts, bindings and source.

The loop is: LLM authors package and business descriptions -> human reviews and
comments -> export anchored Markdown feedback -> LLM changes sources -> reload
and compare. The viewer does not run flows, call models/tools, change executable
source, create graph edges, or install packages on a phone. A proposed prompt or
limit change is a review comment. Direct editing/round-trip YAML export is not
part of this delivery.

## Page and progressive detail

Desktop revision approved 2026-10-24: use the available monitor width and height.
The page is a three-pane workspace: phase/node navigation on the left, an always
visible graph canvas in the center, and independently scrolling details on the
right. Target desktop windows of at least 1180 pixels; a stacked mobile layout is
outside this scope. Long prompts and expanded details must not move the canvas
or create a document-length scrolling page.

- Header: package/revision, open/upload, baseline comparison, validation, export
  feedback. Reload mounted packages explicitly; never overwrite an open review.
- Compact package context: authored purpose and validation stay above the workspace;
  phase navigation stays beside the graph. The right-hand Package tab contains
  the full input/result descriptions, setup, budgets and validation/root selection.
  Facts derived from executable sources include roles, tool grants, mutations,
  routes, completion checks and limits. Descriptions are labelled as authored.
- Graph: auto-layout the current phase/subflow left to right, with fit/zoom/search.
  Open the phase containing the entry step when authored phases are available.
  Begin at readable scale around the selected/entry node; explicit Fit graph
  provides the full overview, while Read selected node restores readable focus.
  A minimap and panning make longer paths navigable without microscopic labels.
  One task card represents a model task; its tools are capabilities, not separate
  model-free nodes. Label verified, route, error, iteration, pause and resume
  edges distinctly, with arrows and separate return/recovery lanes. Adjacent
  nodes outside the current phase appear as labeled boundary cards; selecting
  one opens its phase and inspector. Boundary expansion is capped at 40 nodes
  with an explicit count of omitted connections. Never omit exceptional edges
  without a visible indication.
  Disclose runtime defaults, such as a task's zero-retry failure policy when
  `onError` is omitted, separately from authored transitions. Renderer IDs must
  not collide with source node IDs or impose additional naming restrictions;
  labels, selection and feedback retain the original source identity.
- Subflows and foreach: reveal their body/between references and return behavior;
  allow navigation without expanding every iteration into separate cards.
- Inspector: task definition, base prompt, node supplement, template bindings,
  output/artifact requirements, completion/recovery, permitted tools and arguments.
  Show a static instruction preview with unresolved runtime values identified.
  Node, Package, Source, Feedback and Changes tabs share the independent right
  pane. Choosing a node or source opens its relevant tab; comment actions open
  Feedback. Draft revision/target/excerpt remain pinned until saved or discarded.
- Limits: distinguish generation limits, task-loop limits and flow-frame budgets.
  Explain omission/defaults and actual timeout enforcement. Reserved inactive
  `input.promptTemplate` must be visibly inactive. Package model/setup defaults
  are not necessarily a particular phone's overrides.
- Shared tasks/prompts: identify every affected node. Data inputs link to their
  source where resolvable; dynamic/unresolved values remain explicit.
- Source/details are available on demand; invalid and incomplete packages remain
  inspectable with diagnostics. Unknown formats are never claimed as validated.
  Prompt comparisons retain ordinary multiline text, including quotes and
  newlines, rather than showing JSON string escapes.
- Keyboard-accessible node navigation accompanies the graph in the left pane;
  meaning is not carried by color alone. Large graphs expand incrementally.

Business groupings come from optional `flow-review.json`, supplied by the
authoring LLM. Its references are checked against the parsed graph. Every node
remains discoverable, including unassigned/unreachable nodes. Explanatory text
cannot replace executable facts. See [interface](INTERFACE.md).

## Code boundaries

Use `apps/flow-review/server/` for the independent `trollm-flow-review` Rust
workspace member and `apps/flow-review/web/` for the browser application.
Deployment files live in `apps/flow-review/deploy/`. Public app/operator docs
contain placeholders; private CI integration stays in `ci/` and GitLab YAML.

The server uses existing core plan/registry readers and validators. Reuse the
bundle reader/integrity rules and setup validation from the bridge via a narrow
inspection adapter. Prefer additive APIs or staged temporary installation over
copying a second validator. No executor behavior changes and no native inference
feature are needed. Do not undertake a broad crate split as a prerequisite.

The server returns a versioned inspection document. The browser derives views
and comparisons from it; it does not maintain another execution interpreter.
Use React/TypeScript with a graph library and automatic layout. Keep dependencies
bounded and pinned. Browser rendering is local; Kroki is optional future image
export, not required for the product or hosted service.

## Input and transient storage

Podman pod, initially one non-root application container exposing HTTP 8080.
Serve built browser assets and the inspection API from the same origin.

- Read-only `/packages` mapping: discover package directories and uncompressed
  `.tar` bundles. Do not modify, recursively follow symlinks, or expose arbitrary
  host paths. Root flow and registry are chosen explicitly if ambiguous.
- Upload: existing uncompressed trollm `.tar`, or a browser-selected set of text
  source files with relative paths. No GGUF/models, runtime credentials or SDKs.
- Stage imports in request-private directories under a configurable transient
  directory, default `/tmp/trollm-flow-review`. Remove staged contents on success
  and failure. An upload is an inspection snapshot, not permanent server storage.
- Keep existing archive size/entry/member limits, plus bounded JSON input and
  bounded concurrent inspections. Reject traversal, symlinks, special files and
  duplicate normalized paths. No URL fetching or package-triggered execution.
- Render imported Markdown/labels as text or sanitized Markdown. Do not trust
  imported HTML, SVG, script, filenames or URLs. No arbitrary local file API.
- API and page use one origin; restrictive CORS, request size/time limits and
  normal security headers. Document trusted operator hosting and external TLS/
  access-control integration. This release has no accounts or shared database.

Comments and baseline snapshots live in the browser review session; export is
the durable handoff. Users must be able to download feedback without a live LLM
connection. Do not send packages to third-party renderers or telemetry.

## Delivery and validation

Provide a multi-stage Containerfile and `podman kube play` pod YAML with a
read-only package mount, transient writable directory, resource bounds and
health/readiness checks. Supply a small launcher or clear configuration path
for the mapped directory, published image and port. Runtime requires neither
Android nor model weights. Rootless Podman is the default deployment target.
Use the maintained distroless Debian 13 C/C++ runtime for the GNU-linked server;
the production image needs no shell, package manager or curl. The server's
bounded `--healthcheck` command probes its local HTTP health endpoint. Handle
SIGTERM/Ctrl-C with graceful HTTP shutdown. Container smoke checks actual process,
mount and cgroup state from the Linux runtime host, without adding test-only
application interfaces or untracked helper binaries to the production image.

GitLab builds/tests this component only for relevant inputs or explicit full
pipelines. Run frontend type/lint/tests/build, backend tests/Clippy, HTTP smoke,
dependency/secret checks and HIGH/CRITICAL image gates before image publication.
Use immutable commit image tags, existing registry conventions and repository
CA/proxy setup; do not copy private endpoints into public deployment examples.
Viewer-only changes must not schedule Android/native builds. Add the portable
component/docs to curated export policy, preserving separate public history.

Tests must exercise the real Rust validation path, malformed/incomplete imports,
unsafe paths, presentation coverage, shared prompts, foreach/subflows, routing,
paused continuations, feedback anchors, revision comparisons, and large packages
with many nodes/edges as well as long prompts. Existing examples are smaller
than the requested 300 KB case, so add explicit generated stress fixtures.
Browser tests and an actual container/Podman smoke establish their own scopes;
no phone or live model/MCP end-to-end acceptance is required for this companion.
