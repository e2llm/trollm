# Owned Moray source connector

The source connector is separate from MCP authentication and the executor. It
uses the existing AppAuth authorization-code/S256 and refresh coordinator with a
separate encrypted `trollm_moray_tokens` store. Operator additional roots live at
`trollm/trust/moray-roots.pem`; the platform hostname verifier stays enabled for
source API, issuer discovery and token requests. No redirect forwards bearer
credentials. The source profile contains the publication API, issuer and public
client ID. Existing MCP grants and model setup remain independent.

The Moray source screen selects an explicit publication ID. The client checks
the closed descriptor envelope, complete owner, publication identity, expiry,
availability and supported consumer revision before downloading. `NO_EXPORT`,
revocation, expiry, unauthorized responses and unsupported consumer revisions
refuse. The expected consumer revision is the APK's exact `SOURCE_REVISION`,
set at build from Git or explicit `TROLLM_SOURCE_REVISION`. A fresh compatible
publication is required for positive integration acceptance; an old supported
flag is not compatibility with a different native source revision.

The complete archive must match the descriptor's byte length and raw SHA, plus
the response content headers. A second owned descriptor read checks that the
original owner/identity/availability did not change during transfer. The existing
native bundle importer verifies every manifest member, prompt, registry,
subflow and setup reference and applies core validation. Local installation
recovery reuses retained exact archive bytes and reinspects the native install.
Partial downloads are discarded and retried as downloads, never as executions.
The manifest remains checksum integrity, not an author signature.

Three identities remain distinct:

- `flowSha256` is the exact entry-flow byte SHA.
- `installationSha256` is the exact manifest byte SHA, covering all declared
  files and their hashes. Source imports live at `bundle-revisions/<SHA>`.
- `archiveSha256` is the raw downloaded tar SHA used in Moray reports. Tar
  representation changes can change this hash without changing installation
  contents. No hash is relabelled as a logical/canonical digest.

Manual imports retain their existing root-addressed layout and API. Source
imports do not publish an ambiguous root-only discovery pin. Their setup and
execution use explicit revision APIs through the same installer, model setup,
MCP provider and executor. Native source runs persist `installation.json` and
`source-association.json` before the worker starts. The latter retains the full
original descriptor/owner/grant/install and allocates a native run and one
native attempt identity. Explicit cold continuation keeps those identities and
original dependencies; a later import does not replace them. Missing or corrupt
retained source records refuse rather than selecting a newer install. Source
credentials are never placed in either sidecar.

Fresh source starts recheck the original source entitlement and descriptor.
Existing native resume remains the ordinary explicit resume path. A source
service outage does not itself start or replay a flow. Runtime mutation journal
and unresolved-effect behavior are unchanged.

`MoraySource.retainRunReport` binds caller-supplied actual observations to the
persisted native run/attempt and original publication/archive. It does not
classify model prose as verified website state. The private outbox is written
before POST. `sendReport` retries only the original report under the same source
grant and profile; account replacement or changed report content refuses.
Receipts must echo the whole report and exact native source issuer/client and
remain `device-reported`. Report transport acceptance never establishes a
business postcondition. There is no report-to-execution callback.

The existing run screen provides **Retain native observations** after the
worker stops and **Send or retry retained report**. Retention calls the native
snapshot reader, keeps complete scoped native item mappings and raw evidence,
then writes the immutable outbox before returning its reference to the UI. A
reopened screen loads the retained reference. Send/retry reads that reference;
it cannot call the executor or collect a different report. A new explicit
retention can produce a distinct report if native evidence changed later.

Item identities are scoped native identities, with original item keys and
indices retained in the private mapping. Completed native child records are
`observed`, never `verified` business state. Current unresolved work remains
`outcome-unknown`; untouched items remain `not-started`. A scalar run has no
invented collection items. The source wire outcome remains `unknown` because
this slice has no independent business predicate; the private observation
retains the actual native succeeded/failed status and counters. More than128
items or unavailable nested cursor joins refuse without truncation. Receipt
acceptance does not settle UNKNOWN or extend execution authority.

The first slice implements source access, installation identity and report
custody. It does not add generalized Moray lowering, arbitrary checkpoint
restoration, delegated agents, new collection semantics or external effect
reconciliation. Existing unsupported producer cases stay unavailable. Tests
with deterministic wire responses, stub models and synthetic bundles establish
component behavior only. Real PKCE/refresh, compatible owned publication,
Android lifecycle, configured local models and live browser acceptance require
the separately selected actual workload.


## Declared source admission

Declared text fixture descriptors use a separate closed source branch with
`schemaVersion: moray.declared-native-text.v1` and `origin: DECLARED`. They retain
fixture, prompt and installation identities without inventing browser plan
lineage. A null consumer revision is accepted only for an unsupported declared
NO_EXPORT descriptor with no archive identity or metadata. It cannot install or
start a run. Existing saved-browser descriptors retain their original branch.

The Android declared-input path preserves the original structured JSON before
Kotlin trimming. `admit_source_input` resolves those bytes through the exact
installed setup, rejects duplicate field names including escaped aliases, and
compares the APK's independent normalized JSON against native output as exact
UTF-8 bytes. Normalization uses Rust string trim and Unicode scalar counts;
serialization uses the selected native ordered string map. Different whitespace
or escaping between serializers refuses, even if parsed JSON values compare
equal. Raw and normalized structured input each have their own 65536-byte bound.
This is not RFC8785 or a general JSON Schema input interpreter.

Before a declared run can start, its receipt is recomputed and the installed plan
must be one required text field, one coder task and one successful end, no tools,
connections, subflows, retries or loops. Native limits are one returned-response
step, zero reasoner calls, positive token/second/MiB limits and abort. A failed
generation can have null usage without incrementing returned-response counters;
the plan does not claim a pre-generation universal token ceiling.

The run retains `input.original.json`, `input.normalized.json` and
`input-admission.json` alongside the original source/run association. Explicit
continuation validates those originals rather than replacing them with edited
input. Reporting independently rechecks the retained bytes and receipt, includes
the admission association in native evidence, and never invokes execution.
Admission and byte integrity establish neither independent business success nor
a supported supplier publication. Actual owned artifact preimages, selected
installation profile, source revision and supplier compilation remain separate
joins; no model or APK bytes are implicitly copied into JSON or the flow archive.

## Collection MCP binding boundary

The collection mapping reuses existing prepared-list, foreach snapshot/body/
between/cursor, mutation journal and immutable report paths. The complete admitted
finite roster must independently constrain 1..3 items, original IDs, order and
values; `maxItems` resets per resume and is not that campaign bound. Empty,
changed or subset inputs must refuse before tools. Between-item navigation needs
an explicit last-item guard because the generic between child also runs after
the final item. A committed cursor can continue only under its original input,
bundle and current authority. Declared checkpoint seeding into a new trial is a
separate profile; it cannot be achieved by editing a cursor or replaying a prefix.

The authored native binding uses existing argument constraints rather than model
copies for the admitted complete roster: the selected immutable source supplies
the exact serialized local.prepare_worklist input, and the gateway requires that
whole value before preparing a snapshot. Changed values, order, subsets and empty
lists refuse; generated worklist IDs do not themselves authorize the selection.
`browser` and actual tab/capture identity must be independently bound from current
installation evidence. No source fixture or expected cart is an authority grant.
The changed/subset/empty controls exercise the real local gateway constraint;
existing core recovery controls retain committed cursor and response-loss cases.

A source mapping must identify its browser boundary, complete input roster and
independent outcome predicate. Selecting a source and installation does not
establish native model acceptance. HTTP worker IDs, Relay browser/tab labels, capture SID and MCP
transport session headers remain different identities. Actual current joins and
independent ordered cart/foreign-baseline predicates are required.

`act` is a mutation with live passthrough, intent-log, fail-run uncertainty and no
automatic retry. The bounded result schema
[controlled collection action observations](../schemas/controlled-collection-act.v1.result.schema.json)
can be installed through existing resultValidation. It rejects previews and
indeterminate/unconfirmed dispatch before a task advances. Accepted mutation,
recapture, no-change and navigation variants are returned observations only;
they do not prove business completion or authorize a resend. Authorized preview
continuation is not implemented by this gate. Validation failure after sending
remains conservative uncertainty, with the existing recovery fence.

Refetch-state reconciliation remains unavailable. Lost Add responses stay UNKNOWN
and fence dependent suffix/export, even after later cart readback. Report retry
uses immutable evidence without calling a tool. The actual collection flow,
current session/authority mapping and independent predicate still need their
explicit authored target binding and measured native/model workload. Text and
browser/type components do not replace that goal.
