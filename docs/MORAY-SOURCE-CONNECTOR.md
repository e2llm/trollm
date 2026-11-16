# Owned Moray source connector

The source connector is separate from MCP authentication and the executor. Its
machine path uses an explicitly provisioned client credential in the separate
Keystore-backed `trollm_moray_tokens` store. It does not select a browser, human
password or MFA factor. Operator additional roots live at
`trollm/trust/moray-roots.pem`; the platform hostname verifier stays enabled for
source API, exact issuer token endpoint and issuer JWKS reads. No redirect
forwards credentials. The source profile contains the publication API, issuer
and machine client ID. Existing MCP grants, E2LLM pairing and model setup remain
independent. Historical interactive acceptance is not machine-path acceptance;
existing AppAuth custody is neither imported nor used as a fallback here.

## Machine installation and currentness

The source screen displays its public installation ID. The operator stages one
app-UID-owned regular file at `files/trollm/moray-machine-provisioning.json`,
with owner-only permissions, through the installation's existing private
delivery channel. Credentials must not be intent extras, shell arguments, Git
content, APK assets, logs or runtime/model configuration. A debug installation
can receive the private file through `run-as`; release installations require
their authorized private app-data provisioning channel. No exported credential
import component or auth broker is added.

The closed input has schema `trollm.moray-machine-installation.v1-proposal` and
exact fields `schema`, `api`, `issuer`, `clientId`, `subject`, `clientSecret`,
`installationId`, `owner`. Owner is the independently provisioned six-field
registration/tenant/subject/worker/run/epoch tuple. The token endpoint is exactly
the issuer plus `/protocol/openid-connect/token`; JWKS is exactly the issuer plus
`/protocol/openid-connect/certs`. A token, caller-selected grant/generation,
other-device input or different profile cannot be imported. The explicit
**Import private machine provisioning** button consumes this bounded input
before an atomic encrypted-store commit. It never obtains a token or starts a
publication/model/run. Failed or raced imports require another explicit input.

The store selects one machine principal/profile. Its durable selection revision
also survives removal as a tombstone, fencing a provisioning write that was
already pending. Each provisioning changes the local storage generation. The
authorization grant identity survives ordinary token renewal and same-principal,
same-profile, same-owner secret rotation. Principal/registration/profile changes
create a distinct grant; old installation/report identities are not relabelled.
Removing or replacing the selected principal makes original provider closures
refuse, including after a blocked issuer response. Old MCP/AppAuth state is not
an alternate authority source.

Token acquisition is a verified HTTPS `client_credentials` form POST with exact
scopes `moray:bundle:read moray:edge-report:write`. There is no password grant,
refresh token, redirect or automatic HTTP retry. The response's RS256 signature
is verified against the exact issuer's HTTPS JWKS; issuer, scalar portable
audience, AZP/client, service-account subject, JWT/Bearer types, exact scope set,
issued/not-before/expiry and at most 300s lifetime must match. Tokens remain only
in memory. Concurrent access shares an acquisition; cached age charges the
original full exchange RTT and checks wall/monotonic continuity. These local
checks do not qualify the device's execution-clock assurance.

Token POST and JWKS processing share one original 30s monotonic acquisition
budget. Continuing response progress cannot restart that bound. Socket timeouts
are capped by remaining time, and cancellation interrupts the acquisition and
closes its current connection. One shared 1s cleanup allowance checks actual
owned worker/closer termination and connection closure; cancelling a Future is
not retirement evidence. Unconfirmed teardown permanently fences that provider
instead of admitting another request or a late token. Cancelled consumers can
leave the interruptible provider-lock wait. Source-screen, report-send and
fresh-start coroutine paths bridge cancellation to the blocking acquisition;
no execution start is wrapped in a replacement worker or retry loop.

Fresh source reads, downloads and starts retain original grant/generation checks
around asynchronous I/O. Each descriptor's complete owner must match installed
registration. Retained reports keep their original grant/outbox bytes across
token renewal; receipt issuer/client and exact machine subject are checked.
Report retry only sends the retained body, never execution. A replacement
principal cannot adopt an old report by supplying its ID.

Local credential removal is immediate local refusal. The API validates JWTs
against its installed offline keys/policy. Disabling the issuer client or
rotating its secret prevents future issuance but can leave an already-issued
token valid until its original expiry, bounded by 300s under valid clocks.
Earlier API refusal requires an explicitly installed restrictive policy/key/
subject mapping and reload. Issuer signing-key changes likewise require a
coherent API key update. No instant remote logout or registration mutation is
inferred from a local storage operation.

The machine source batch is not yet qualified on an APK, actual issuer/API or
device. Declared/JVM controls, source review and historical interactive results
must retain their separate scopes. Source preparation does not establish
deployment or acceptance of the passive-report integration.

## Source and report custody

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
retains the actual native succeeded/failed status and counters. More than 128
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

## Native preparation adapter source boundary

The native preparation port runs after typed/model argument binding and before
final validation and mutation intent. Its evidence retains original proposal,
requested bindings, exact resolved call and genuine native context separately.
Selected gateway tools require a single-use preparation; direct missing, changed
or reused preparation refuses. The final transport check follows credential lookup
and the client lock. Cancellation after a possible send does not retract it.

The private HTTPS embedding adapter performs read-only guard calls and retains
its once-generated response bytes. Original native u64 context remains opaque
across JavaScript, with a separate lexical projection. This transport is not an
installed RPA selected-profile bridge, owned admission or a business verifier.
The executor post-result notification follows the intent-linked result event and
artifact processing; it does not itself authenticate journal records, classify a
Relay disposition or acknowledge worker inactivity. The selected-provider source composition is described below; it is not installed
by the Android run service. Existing journal durability assumptions remain unchanged.

The selection reader checks the exact vendored eb093 schema, raw canonical bytes,
external reference and local relationships. It does not establish current owner
authority, resolved original/changed source relations or restoration fidelity.
Selected policy/oldest-observation timing, original expiry evidence, exact outcome
acknowledgement recovery have source controls below. Genuine selected-invocation
installation and stopped-worker completion still require the coherent profile join. Navigation remains unsupported without a real
pending observer acknowledgement before the native send. Source-only preparation
and focused controls do not qualify a collection runtime or any model workload.

The private outcome transport consumes the closed RPA acknowledgement contract.
Within the same
adapter lifetime and original cutoff it retains received HTTP request bytes and
returned ACK bytes, repeats that exact ACK only for identical received bytes,
and fences conflicts. This is separate from RPA-generated JSON.stringify evidence.
Restart/cold recovery is unavailable; no new output directory recovers by filename.
The transport does not authenticate native journal contents or classify effects.
A bounded native journal extractor preserves original intent/result JSON record
text, validates the whole journal and exact call associations, and excludes JSONL
delimiters explicitly. It is not result-artifact verification, a signature, a
business verdict or a completed selected provider. Those joins still refuse.

The selected result reader requires live provenance, complete native-owned
artifact metadata, exact unshaped parsed-envelope equality and a definite Relay
observation variant. Preview, uncertain/unconfirmed/partial/error and cached
results cannot acknowledge a selected effect. No-change is not proof of no effect.
The provider still must bind the artifact bytes/hash to original journal evidence.

Native clock mapping uses Linux/Android CLOCK_BOOTTIME plus the actual boot ID,
not an Android wall-clock offset or cross-process monotonic subtraction. A runtime
anchor can conservatively map an authenticated verifier UTC sample acquired within
an actual request bracket by charging the full round trip and verified error,
then adding local suspend-aware elapsed time. Discontinuity or expiry refuses;
the anchor is not persisted/restored. The installed verifier must authenticate
sample provenance, acquisition and validity before construction. The arithmetic
helper does not establish that assurance or make check-and-act atomic. Unsupported
platforms or unavailable boot identity refuse. Host clock controls are not an
actual Android suspend or qualified installed-verifier test.

The native retained-outcome builder revalidates the immutable selection bytes,
original run/context/intent/result linkage, the complete live parsed-envelope
artifact and its later native artifact-written hash/size record before constructing
the closed RPA outcome. Its generated evidence envelope explicitly labels original
record text and parsed artifact bytes, stays within the decoded16KiB wire bound,
and refuses missing custody or drift. The HTTPS ACK method checks the exact closed
ACK and never automatically retries. These methods still need selected installation,
current-authority/final-clock and genuine worker-lifecycle wiring; their existence
is not a runnable or supported collection profile. RPA's measured validUntil is
retrospective coverage and cannot supply forward native mapping validity.


The selected provider now composes the literal RPA72 source contract in native
code: exact ready/guard v2 association, admitted item and original opaque native
context, original cutoff evidence, oldest acquisition/error arithmetic, final
arguments and the installed result-artifact destination. The task-level
resultArtifact option remains read-only; a mutation uses its installed tool-policy
artifact destination. Native original journal and complete live artifact custody
precede the definite outcome report. Report request/ACK files retain exact native
HTTP bytes separately from RPA-generated object serialization. Controls use a real
loopback TLS boundary with explicitly controlled authority/clock evidence, not a
real installed browser, current owner or Android assurance.

A successful selected preparation remains pending across its durable intent,
retained result and reporting. Only a retained outcome acknowledgement clears it.
ACK loss refuses the next action, partial success and execution replay, including
journal reconstruction. A returned tool response alone does not change the
selected mutation from UNKNOWN to returned. Reporting recovery does not resend
an effect. Worker status now reports the actual worker-active flag even when a
terminal phase has been published during teardown; terminal phase alone is not
inactive-worker acknowledgement.

The provider requires native implementations of current authority and a bounded
same-device UTC upper bound. Neither has a JSON flag/default implementation.
The production Android run service does not yet install this provider or bind a
selected invocation to its worker. Collection source admission and the supplier
node/task/cursor mapping are also unfinished; the existing DECLARED text profile
is unchanged. Missing installation still refuses rather than admitting a model or
caller-supplied assertion as authority.

Selected prepared calls use a fresh strict TLS/HTTP1 connection within the existing
native transport. After DNS/connect/TLS readiness, the socket waits for writability,
rechecks the exact credential plus native intent/arguments/authority/clock/cancel,
and performs the nonblocking first request write. WouldBlock or interruption before
an accepted byte repeats readiness and the check. This covers HTTP headers and
body, with no redirect, address retry, pool reuse or effect retry. Partial/possible
writes remain UNKNOWN. Legacy reqwest calls retain their pre-method check and do
not establish this first-write capability. The selected transport requires a direct
HTTPS connection; proxy-only and other platform profiles are unsupported.

Focused controls exercise delayed real TLS readiness with cancellation/expiry,
wrong CA/hostname refusal, exact 40KiB body delivery, credential rotation during
handshake, real socket backpressure, bounded/ambiguous response framing and one
mutation followed by reply loss. The TLS and socket-backpressure controls are
separate component cases. OS DNS resolution can outlive the cooperative deadline;
a late resolution cannot authorize a write, and worker-inactive status must not be
reported while that native worker is still running. None of these controls proves
an atomic check-and-effect, future clock health, Android installation, business
predicate, navigation observer or full workload acceptance. Production native
clock/current-authority and selected worker installation remain unfinished.


The native kernel reader vendors an exact RPA source/header pair. Its
[source manifest](../crates/trollm-mcp/vendor/rpa-kernel-clock/source.json)
records the private upstream revision and raw file hashes; the
[notice](../crates/trollm-mcp/vendor/rpa-kernel-clock/NOTICE) records the licensing grant.
The Cargo build compiles that unchanged C source with
MORAY_KERNEL_CLOCK_NO_MAIN and no syscall override. Both C and Rust assert the
fixed-width 32-byte ABI and every field offset using the actual target compiler.
The existing cargo-ndk invocation supplies the Android compiler; no separate JNI
helper process or clock service is introduced. All source/header/layout changes
select the native consumers and Android CI lane.

Rust preserves the C return code and raw state/status/synchronization/error
fields, and checks only an explicitly supplied error ceiling. Controlled error
cases and a real linked host read exercise that boundary without assuming host
synchronization. These checks are not NDK/device qualification or an installed
final-send clock implementation. The reader supplies no timestamp, forward lease,
clock-rate bound, authority, or Android suspend proof. The qualified RPA72 host
installer remains attributed to its original source; this acquired native reader
is a separate explicit source pin.


The executor's after_retained_intent hook now precedes selected dispatch. The
provider validates the complete original native journal, exact preparation/context
and final argument linkage, then retains separate hashes of the original prepared
and intent record text. Final-write and post-result checks require those same
original bytes, not a reserialized logical equivalent. A direct prepared gateway
call without this association refuses before effect dispatch; intent failure,
abandoned preparation and guard failure fence subsequent attempts. The generic
executor still validates arguments without network I/O before recording intent.
These source associations are not cryptographic signatures or a caller-provided
proof of ownership. Actual installed source, authority and worker joins remain
required before profile support.


Before the selected first write, the provider also reads the existing authenticated
private /control endpoint. It requires the original invocation and unchanged actor
cutoff and refuses abort; the final authority/time check follows that I/O. The
control read is adapter liveness only, not a native inactive-worker acknowledgement.
Selected actor expiry must equal the minimum of the original case, selection and
authority cutoffs. Empty or already-complete selections can have no action bindings;
any attempted effect then refuses. These constructor/source cases do not establish
restoration, owned admission or an executed empty workload.

The private JavaScript wrapper now recognizes the explicit selected-v2 prepare
shape. It checks the inner opaque caller-context bytes against the outer original
native bytes without parsing native u64 values, preserves the separate association
bytes/hash and refuses missing selected input instead of using the legacy one-item
request. It does not authenticate native journals or assign business outcomes.

Machine-token failures use closed exchange/claim stage codes, including response,
JWKS, encoding, key/signature, identity/scope and issuance/not-before/expiry/lifetime
boundaries. They never include raw tokens, claim values or exception payloads.
Strict issuance and not-before checks remain; no clock-skew allowance or selected
execution-clock substitution is implied. Warm-cache/refresh and cold-provider
controls preserve the original grant lineage and exchange RTT/expiry reserve.
An observed warm failure followed by a cold success is not attributed to clock
offset without the actual closed boundary evidence.
