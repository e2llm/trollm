# Flow review implementation and checks

The companion is implemented in [apps/flow-review](../../apps/flow-review/README.md).
Read the [design](DESIGN.md) and [interface](INTERFACE.md) before changing its
behavior. These documents cover the review application; they do not change
trollm's execution-plan format or establish Android product readiness.

## Implemented scope

- An independent Rust inspection server and React/TypeScript browser application.
  Android and the executor do not depend on this component. The server reuses
  current plan, registry, bundle-integrity and setup validation.
- Read-only mapped packages, uncompressed bundle uploads and source-file/folder
  imports. Uploads use bounded request-private staging and are removed after
  inspection. Unsafe paths, symlinks, special files and oversized inputs reject.
- Authored business purpose and phases, checked against actual node references.
  Unassigned nodes remain visible; descriptions cannot replace executable facts.
- Graph and keyboard-accessible list navigation, node/task details, prompts,
  bindings, tool grants, limits, completion, routing and recovery. Runtime
  defaults and unresolved dynamic values are identified.
- Comments anchored to source identities and revisions, downloadable Markdown
  feedback, and before/after comparisons of executable fields and prompt text.
- A Podman pod with a non-root distroless image, read-only package/root mounts,
  transient storage, memory/CPU limits, readiness checks and graceful shutdown.
- Authoring guidance and a checked business-description example in the
  [Check a total starter](../../flows/starters/check-total/flow-review.json).

Authoring stays with an external LLM. The viewer does not run flows, call models
or tools, edit executable sources, or install packages on a phone. Proposed
changes are comments for the authoring LLM to implement and validate.

## Validation performed

| Check | Scope established |
| --- | --- |
| Rust unit/integration tests and Clippy | Canonical validation, invalid/incomplete inputs, bounded imports, safe paths, presentation references, health probes and shutdown. |
| Browser type/lint/build and unit tests | Graph/inspector data, feedback anchors, comparisons, defaults and dependency notices. |
| Chromium fixture workflows | Import, navigation, comment/export and revision comparison, including a 1,500-node inspection. |
| Browser against the real HTTP server | Mounted Check a total inspection, phases/prompts/graph, downloaded feedback, changed-prompt comparison and starter tar import. |
| Generated large source package | Inspection of a package exceeding 300 KB with 402 nodes. |
| Actual rootful and rootless Podman smoke | Imports, assets, health, effective non-root UID, real read-only mounts, tmpfs, kernel resource limits, cleanup and graceful shutdown. |
| Dependency, secret and runtime-image scans | The tested runtime image had no selected HIGH/CRITICAL findings; Rust and browser dependency checks are separate. |

The Podman checks used Linux, systemd/cgroup v2 and Podman 5.8.2. The rootless
host already had subordinate UID/GID mappings and delegated resource controllers.
See the [deployment guide](../../apps/flow-review/deploy/README.md) for required
host permissions and the portable smoke command. Host/container checks do not
establish behavior on every Podman host or browser.

No phone, native inference or live MCP execution is required for this companion
or was used for its acceptance. The browser's fixture tests and real-server
checks establish different scopes. Validation of package structure and hashes
does not establish the accuracy of model prose or business descriptions.

## Continuing development

Start from [AGENTS.md](../../AGENTS.md) and the component's
[development commands](../../apps/flow-review/README.md#development-and-checks).
Keep server, browser and deployment changes within this component where possible.
Preserve the versioned inspection/presentation contracts and keep runtime
semantics authoritative. Coordinate consequential design changes with the
maintainer.

Run checks for the changed layer. Viewer-only changes need no Android SDK,
model weights or native inference build. Recheck real HTTP/browser integration
when the inspection contract changes and actual Podman behavior when image,
launcher or pod configuration changes.

Reviews live in browser page state; export feedback before leaving or reloading.
The service has no accounts, shared review database or multi-tenant boundary.
Use a trusted host and external TLS/access control when exposing it over a
network. Validation covers the selected entry flow and referenced subflows;
unused supplied flows stay visible with warnings.
