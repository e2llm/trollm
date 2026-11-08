# Native collection member offer

`collection-members` is the target-owned executable member builder. It emits the
existing `ledger.execution-plan.v1`, `ledger.task-registry.v1`,
`trollm.flow-setup.v1` and `trollm.flow-bundle.v1` formats. It neither publishes nor
installs a source, reads credentials, starts a worker or sends an effect.
The proposed distinct source name is
`moray.declared-native-collection.v1-proposal`; this does not extend the existing
text-only publication decoder. The member profile is
`trollm.native-collection-members.v1-proposal`.

Build/use the existing Rust toolchain:

```sh
cargo run --locked -p trollm-uniffi --bin collection-members -- \
  /absolute/selection.json /absolute/selection-ref.json /absolute/owner.json \
  /absolute/context.txt https://api.e2llm.dev/mcp /absolute/new-members
```

All files are bounded regular inputs. Selection bytes must be the exact canonical
Toolbox eb093 selection body with its external raw-SHA reference and full owner.
The context bytes must match `contextRef.sha256`, be UTF8 and fit 2048 bytes.
The context is source data, never an evaluator's expected result. The whole
selected roster is bounded to three entries. Each original/changed/subset/empty
body is separately retained; shape validity does not prove its resolved source or
changed-baseline relation. Toolbox owns those current-owner/source checks.
Complete original/changed bodies must retain their own contiguous request
positions. Subsets must retain increasing original positions and be smaller than
the three-item domain. For a smaller source domain, the producer must independently check the source
count and proper subset relation; a label cannot supply either.
The endpoint is explicit: the paired API or MCP DEV endpoint, with no fallback.
Output must be a new absolute directory. Stdout reports only member identity,
real loaded plan pins and derived task bindings, with runtime support false.

## Exact generated members

- `selection.json`: original canonical complete admitted selection bytes, no LF
  added and no prefix removal.
- `context.txt`: exact resolved source context bytes.
- `prepared-worklist.json`: canonical serialized `{context,items}`. `context` is
  a string containing the selection ref/raw SHA, selection/case/invocation,
  original context ref/data and known-prefix boundary. `items` is exactly the
  new suffix; each element retains all original `originalItemId`,
  `inputPosition`, `admittedOrder`, numeric `quantity` and string `quantityText`.
- `setup.json`: coder-only pinned Qwen3.5-0.8B Q4_K_M, ctx4096/temp0.2/topP0.95,
  one explicit MCP bearer connection, one required `selectionSha256` text field
  capped at64 characters, and a fixed full serialized worklist argument. The
  full selection stays in its member rather than exceeding the existing16KiB
  field cap. Native input admission must match the exact complete selection SHA.
  Model bytes/runtime path/chat-template custody remain separate dependencies.
- `selected-collection.flow.yaml`: one local worklist task followed by snapshot
  foreach `items`, source ordering, native `id` key, explicit body aggregation
  and empty/complete exits. The complete selection is the immutable hard bound;
  there is no per-resume `maxItems` campaign bound.
- `selected-item.flow.yaml`: candidate -> quantity -> Add -> end. Body input
  `item` comes from the native worklist wrapper's `value`; `context` comes from
  the prepared worklist. These are genuine frame inputs, not model transcription.
- `selected-collection.tasks.yaml` and `prompts/*.md`: bounded single-result
  tasks. Quantity binds `value` from `item.quantityText`; actions are fixed.
  Every act task is mutation/intent-log/fail-run, with no retry, routing or
  inspect-as-verifier. `modelFields: []` forbids invented browser/SID/target.
  Installed mandatory preparation owns those wire fields and the model schema
  projection omits them; final resolved wire validation still applies.
- `collection-profile.json`: explicit source/member proposal, raw selection and
  worklist associations, cursor/task purposes and unsupported boundaries.
- `manifest.json`: every other member's raw SHA, including setup, prompts,
  context and full selection. Its own raw bytes determine installation SHA;
  archive and entry-flow hashes remain distinct.

Target member bytes use native serde JSON plus one LF, valid YAML where named
`.yaml`. Selection/worklist canonical input bytes use the producer's separate
canonical basis. Do not equate logical source identity with either raw hash.
The server emitter may reproduce these members and package them using its
existing archive path; `CollectionMembers::inspect` rederives exact bytes and
uses the real native bundle/setup/registry/flow validators before deriving keys.
No caller supplies a synthetic native plan pin or item ID. `local.prepare_worklist`
creates native `{id,value}` records using its existing actual implementation.

## Continuation and admission limits

Empty and complete-prefix selections use an explicit end-only root with an empty
registry; they do not call the existing nonempty-only prepare tool or invent a
cursor. This is zero allowed work over a separately supported nonempty source
application, not an empty-application qualification.

`inspect` returns actual root/body plan pins, action node/task/purpose bindings
and the expected native suffix snapshot. The preparer checks the root frame,
actual in-body cursor, exact complete JSONL suffix values and original native
`artifact_written` record before preparing an action. Missing, extra, reordered,
rewritten or unrecorded snapshots refuse. Known-prefix parent/restoration custody
still requires its actual independent source relation. It is not execution credit.

Runtime admission remains unsupported until the real installed owner/authority,
SameDeviceClock, invocation-to-worker and mandatory preparer bindings exist.
The proposed source union must be agreed with the producer and implemented in
both Kotlin and Rust; text-only admission remains intact meanwhile. The public
profile flag is a description, never a way to enable support.
Multiple suffix items also require a genuine pending-before-send navigation hook
and real N-1 transitions. No fake between flow or same-page repeated Add supplies
that missing behavior. The builder can emit/inspect finite representation cases
without claiming they execute a qualified campaign. Lost/partial sends stay
UNKNOWN, no next action/resend; report-only retry never repeats an effect.
Independent full-cart/foreign-baseline verification stays outside these members.

## Additive source-union offer to Toolbox

`CollectionSource` and the closed [source schema](../schemas/declared-native-collection-source.v1.schema.json) define the target-side source projection proposal:

| Field | Required basis |
| --- | --- |
| `schemaVersion` | `moray.declared-native-collection.v1-proposal` |
| `origin` | `DECLARED` |
| `owner` | Actual full six-field portable owner |
| `fixtureId`, `versionId` | Actual supplier-assigned immutable source identities |
| `selectionRef` | Actual same-owner retained canonical complete selection |
| `selectionRawSha256` | Exact `selection.json` bytes, also equal to ref SHA |
| `profileId` | `trollm.native-collection-members.v1-proposal` or the distinct matched `trollm.native-collection-members.v3-proposal` |
| `profileSha256` | Exact `collection-profile.json` member bytes |
| `installationSha256` | Exact `manifest.json` bytes |
| `qualification` | `unqualified` |

Here `source.profileId` names the target member profile. It is distinct from
`selection.source.profileId` (the original source profile) and the private RPA
`nativeProfileId` (legacy `controlled-collection-native-selected.v2-proposal`,
or matched `controlled-collection-native-selected.v3-proposal`).

The source projection deliberately has no synthetic `planSha256`, recorded
provenance, model-success claim, expected cart or runtime support switch. The
outer existing publication envelope retains archive/entry-flow/manifest identity,
metadata, actual consumer revision, original created/expiry and availability.
Its manifest identity must agree with the source installation hash. An unavailable
new source branch must remain unavailable; the current text branch is unchanged.
The Android decoder recognizes this closed source union and retains unsupported
publication refusals. Recognition does not establish serving API adoption or
native execution admission.
`CollectionSource::inspect` is executable source/member checking, not current
owner authorization or an installed dispatch entry. It checks complete original
selection/raw refs, context, profile and manifest then rederives the actual members
and native plan/task/cursor bindings. Toolbox must pair this proposal with actual
custody/source relations and emit real IDs; no example ID becomes owned authority.

## Installed authority composition

`collection_authority::ApplicationSelectedEnvironment` implements the native
environment port using the original source association, selected native run,
app-owned authenticated publication checker and `SameDeviceClock`. Its
constructor is native composition, not a JSON support flag. The Android checker
uses `MoraySource.authorizeFreshStart`, including original grant checks before
and after the actual HTTPS descriptor read. Source credentials stay in that
client and remain separate from MCP credentials.

Acquisition clears old authority, samples the actual device before and after
the read and retains the oldest conservative lower bound with the authentic
native-call context hash. Final local checking makes no application/network
callback: it refuses missing, pending, different-call or stale acquisition,
samples the device again and checks original publication expiry. The explicit
selected age ceiling is never renewed by final checking. These are bounded
observations, not atomic remote revocation or check-and-act guarantees. Host
retrospective clock evidence cannot replace the device samples.

The Rust embedding entry `run::spawn_installed_source_run` accepts an out-of-band
`SelectedRunInstallation` and binds this environment and the inspected preparer
to the genuine run before worker startup. The worker installs that instance in
its per-run gateway before model setup or dispatch. The ordinary
Android `start_source_run_configured` entry still supplies no installation and
refuses collection execution. The explicit Android private-installation/FFI
entry is described below and has not been installed/qualified. Native raw/normalized input admission must agree with
the exact inspected `selectionSha256`; changing to another independently valid
input cannot redirect the retained roster.

The explicit installation carries the private ready object, authenticated guard
client, selected device error ceiling and SHA of the exact selected MCP key
string's UTF-8 bytes. This is not the key file, Authorization header or source
credential hash. The actual installer must establish its association with the
selected RPA account/transport; the hash alone does not establish account
authority. No credential or private ready object enters member/model input or
the public source association. Selected static-key authentication pins the
original endpoint/mode/key and protocol, permanently fences observed loss or
replacement, and installs mandatory preparation for `e2llm.act`. Ordinary manual
authentication cannot silently become that selected credential binding.

Startup requires actual source and actor checks even for an empty/end-only
source. A create-new, synced owner/invocation launch claim binds the genuine
native run and exact selection/installation. Identical fresh starts, changed
bodies under that invocation and partial claims refuse. This reservation is not
an effect intent, delivery receipt or worker-stop proof. Failed startup may leave
an unstarted directory/reservation; it never authorizes retrying an effect.
Cold selected reconstruction remains refused without the original actor/custody
recovery. In-process continuation retains the original prepared instance and
complete finite snapshot bound.

Effective runtime configuration must retain the admitted coder hash/context/
sampling values and inspected MCP endpoint. Its actual path, model bytes,
installation and owned artifact/read-basis evidence remain separate obligations.
The native cooperative control resamples the original actor cutoff, including
while held or awaiting authored continuation. Its 100 ms wake cadence is an
implementation detail, not a freshness default or hard-stop guarantee. Scheduling
or platform blocking can delay teardown; only the actual `WorkerGuard` clears
the worker-active flag.

The legacy v1/v2 installed embedding still refuses suffixes longer than one
before any effect. The explicit matched v3 constructor below adds source-level
multi-item admission through inspected members and a distinct ready/guard tuple.
A matching RPA v3 runner and actual installed Android/runtime qualification
remain required; source admission does not establish N-1 execution acceptance.


### App-private Android entry and worker readback

`SelectedNativeRun.start` uses the generated UniFFI
`start_selected_source_run_configured` entry, not the ordinary source-run opt-in.
It requires an explicitly provisioned file under the app runtime's
`selected-installations/<raw-file-sha256>.json`. The parent is app-owned mode
0700; the bounded regular file is app-owned mode 0600. Symlinks, traversal,
source/invocation mismatch and changed bytes refuse before startup. The closed
`trollm.private-selected-installation.v1` file has exactly `schema`,
`installationSha256`, `ready`, `guardUrl`, `guardToken`, `guardRootsPem`,
`selectedDeviceMaxErrorUs` and `expectedStaticKeySha256`. The selected launcher source below provisions actual private
ready/credential/trust/policy inputs through the app UID filesystem channel. This file is never a publication member or model input.
The hash identifies original bytes; it does not prove launcher provenance,
exclusive credential custody or account authority by itself.

The native token getter rechecks the original private installation bytes before
private transport acquisition and permanently fences an observed loss/change.
The returned native run ID binds a separately retained private-installation
reference and invocation claim before the worker starts. `selected_run_binding`
returns original claim/source association JSON as strings plus the actual
same-process worker-active flag. Missing/evicted handles refuse rather than
claiming an inactive worker from a cold directory. The Android handle checks its
original invocation and installation key and delegates cancellation to the
existing runtime. It introduces no effect sender or progression loop. Callback
return, timeout and outcome ACK remain separate from actual worker inactivity.

The internal handle is consumed by the private debug launcher below. A source
implementation does not establish an installed launcher or actual Android
provider qualification. RPA still performs its own receipt and post-observation
checks after the native retained-result outcome. Local controls do not establish
a positive device or N-1 execution.

### Private debug launcher composition

`scripts/android/selected_native_launcher.cjs` exports
`withSelectedAndroidDriver(options, useNative)`. It embeds the existing
`openGuardAdapter`; it has no MCP effect sender or agent progression loop. Call
it inside the maintained RPA external-driver callback with the original `ready`,
`guard` and `signal`. The RPA source pair is the complete `49cfec888c9e675198b7abcd3f67283f8447cc31` precursor,
not an overlay of its helper into a different runner. Its pending navigation
broker successor and matched v3 ready/guard remain required. The legacy runner
does not acquire navigation admission from helper fields or this launcher.

Options include explicit ADB executable/server host/port/serial, the expected
single debug APK's raw SHA/size/source revision, original selection reference,
flow/installation hashes, original admitted input/configuration/association
strings, selected device clock error ceiling, expected MCP key digest and
operator-supplied private HTTPS key/certificate/root PEM. Inputs remain private;
the selected guard credential is generated inside the launcher and is never a
model/source member, intent extra or ADB command argument. The source authority,
MCP credential and same-device clock checks remain native-owned.

The launcher checks the actual serial, boot UUID and streamed installed APK
bytes, refuses split APKs and mounts one no-rebind ADB reverse for the private
HTTPS endpoint. Through `run-as com.trollm.app` stdin it creates bounded,
no-clobber app-private installation and request files, then verifies readback.
It does not install APKs, import publications, issue credentials or alter app
authentication. This profile requires the operator's existing private debug
ADB capability; it does not authenticate a remote operator solely from a hash.

The existing MainActivity dispatches only the request-file hash to
`SelectedNativeLauncher` when the actual app is debuggable. No secret or arbitrary
JSON is accepted from the intent. Release apps ignore this action. The app checks
its private directory/file custody, exact original request bytes, installed APK
hash and compiled `SOURCE_REVISION`. Create-new request and invocation claims
precede the one selected native start. Changed/repeated starts cannot reconstruct
or start another worker. The genuine returned run ID is retained before binding,
history or foreground-service operations. The process-held `SelectedNativeRun`
then supplies the original native claim/source strings and actual worker flag.
Cold/missing handles refuse. Result files have a separate completion marker so
partial writes cannot appear as complete replies.

Guard requests wait until this same app's returned native installation/run/
invocation binding has been checked. The credential is shared only between the
trusted operator launcher and the selected app-private installation, and the
native pinned token getter rechecks that original file. That provenance is the
trusted installed-provider boundary. It is not a standalone journal verifier:
`after_retained_result` still authenticates original native intent/result/artifact
records before sending an outcome through the existing `PrivateGuardClient`.
An arbitrary payload, matching digest or bare `OBSERVATION_RETURNED` is not a
replacement for that installed implementation and exclusive credential custody.

`useNative(handle)` receives `binding()`, `cancel()` and `waitInactive()`. These
use the same process-held handle and original invocation. The callback's own
native progress report/intent accounting remains separate; returning from it
cannot prove stop. Completion is withheld until actual `workerActive:false`
readback. On failure the launcher cancels once when a genuine ID is available,
including recovery of the app-retained ID after a lost start reply. It never
repeats start or an effect. Missing ID, process loss, changed boot or absent stop
readback remain unconfirmed. Cleanup has a bounded local cancel/readback interval
that renews no effect authority. It closes its own reverse/HTTPS resources and
retains a private closure receipt; root still owns finite actor/device retirement.
The final error and closure retain sanitized `primaryFailure` stage/code and
child/native classifications separately from all `cleanupFailures`. Unconfirmed
worker stop remains the mandatory final verdict without erasing a provisioning,
start or caller failure. Reverse and adapter-close failures are collected before
the closure is written; a close exception cannot bypass that verdict. A failed
cancel reply does not skip independent worker readback. Raw messages, stderr,
model input and native payloads are not copied into these diagnostics.

Node controls exercise real subprocess, stdin/readback, APK stream and HTTPS
adapter boundaries with declared Android response fixtures. Kotlin closed-command
controls are authored for the normal Android JVM lane. These are not actual
Android/provider provenance, device clock or navigation execution evidence. The
new app path still requires same-source UniFFI/Android compilation and installed
qualification in the single coherent candidate; the older bc994 APK lacks it.


### Explicit N-1 member offer

`CollectionMembers::compile_n1_offer` and `inspect_n1_offer`, or the existing
member CLI with `--n1-offer` before its six arguments, emit and validate the
separate `trollm.native-collection-members.v2-proposal` offer. The default v1
recipe bytes are unchanged. The public source decoder still refuses this v2
offer; matched consumption uses the separate v3 profile below. Its original
runtime and navigation support flags remain false. This is an
executable member/lowering offer, not installed navigation admission.

For a nonempty suffix, root `items.between` references
`selected-between.flow.yaml`. Its `choose` node uses the existing
`choose-continuation` task and `flow.choose` routes `next` to `navigate` and
`final` to `done`. The existing executor supplies `items.cursor` as the completed
suffix count and `items.snapshot_len` as the immutable suffix length. Between
executes after the final body too. The model route must select final there; it
is not a trusted predicate, independent verifier or navigation permission.
Native preparation must independently reject any navigate action at that final
cursor, even if the model requests it. An early final route also does not prove
a valid next page or full collection outcome.

The `navigate` node's task is `navigate-next`, with semantic preparation purpose
`navigate-next`, fixed `action: navigate`, no model-owned arguments, and the same
mutation intent-log/fail-run/no-retry policy. It has no fixed destination URL.
The matched RPA guard must supply exact source-computed Relay arguments after
arming its existing listener. New member inventory adds that between flow and
`prompts/choose-continuation.md` and `prompts/navigate-next.md`; the complete
selection and explicit suffix worklist bytes are preserved. Empty/complete-prefix
cases still emit no between task or effect.

The remaining matched runtime contract must bind the genuine inBetween frame,
completed-item cursor and exact between plan/task pins, enforce the independent
last-item cutoff before intent/send, intersect the original RPA listener arm
expiry into preparation expiry, and await the authentic native outcome plus the
actual navigation transition before progressing. This offer does none of those
runtime checks by pretending that member serialization proves them. RPA's
published scoped adapter primitive remains distinct from its unfinished selected
runner mount and actual installed launcher provenance.


The explicit RPA791b122 mapping separates the namespaces: native operation
purpose `navigate-next`, fixture purpose `catalog-link`, and Relay action
`navigate`. Native node/task names remain `navigate`/`navigate-next`. The guard
must return fixturePurpose `catalog-link`; the native operation name is not a
DOM data-control alias. The returned Relay wire uses `target` for the next URL,
never a new `url` field.
The DOM selector/node and inspected catalog-link href are separate observation
proof and are not the action target alias. The source-computed next URL comes
only from the already armed RPA callback. Native cursor association uses the
just-completed item, while knownCommittedPrefixCount is admittedOrder + 1.
Native source controls now reject a wrong cursor phase/final-item navigation and
check the original fixture origin and next page separately from the DOM selector.
The matched v3 constructor makes these validators reachable only through its
explicit inspected-member/private-installation admission. The legacy path stays
fenced; matching runner and installed provenance/qualification are still required.

Effective preparation expiry may be shortened below the original age/authority
minimum, for example by the original navigation arm minus verified source clock
error. It may never exceed that minimum. Preparation reserve still applies at
preparation completion only; final write checks the retained original shortened
expiry without granting a fresh reserve or replacement duration.


Retained navigation outcomes require the existing Relay result/classification
pair `navigation`/`navigation` and an original native intent whose actual sent
action is `navigate`. A navigation observation cannot acknowledge a click/type
intent, and a mutation observation cannot acknowledge a navigate intent. The
same original intent/result/artifact custody checks still run first; previews,
errors, partial/unknown/lost replies remain refused. Even this definite returned
observation does not establish the page transition or stopped worker: those
remain the matched RPA postcheck and genuine native lifecycle obligations.

All four definite result variants require their canonical existing
`actionClassification` partner, as defined by
`controlled-collection-act.v1.result.schema.json`. Missing or contradictory
classification withholds an outcome ACK rather than treating a successful HTTP
response or a single result label as delivery proof.


### Matched installed pending profile

`compile_matched` / `inspect_matched`, or the existing member CLI with
`--matched-pending`, use `trollm.native-collection-members.v3-proposal`. They
retain complete original selection and explicit suffix worklist bytes and the
existing N-1 flow/task lowering. V1 and the historical v2 offer bytes are not
modified. V3 has distinct profile/manifest hashes and declares its required
installed boundary rather than treating a source support boolean as permission.
The public closed source DTO accepts v1 or v3, never the historical v2 offer.
The compiler CLI still reports runtimeAdmissionSupported:false because it only
materializes/inspects bytes and performs no installed execution.
Android recognizes the same two formats and still respects NO_EXPORT, current
owner/source authorization and immutable installation checks.

The exact matched tuple is:

| Layer | Required discriminator |
| --- | --- |
| Member/source `profileId` | `trollm.native-collection-members.v3-proposal` |
| Private ready `schema` | `controlled-collection-selected-ready.v3` |
| Private ready `nativeProfileId` | `controlled-collection-native-selected.v3-proposal` |
| Returned guard `schema` | `controlled-collection-selected-guard.v3` |

`InstalledCollectionPreparer::new_matched` requires the inspected v3 member
closure, actual root/body/between pins, complete new-suffix snapshot and all four
action bindings (zero for empty/complete-prefix). The installation factory
requires source and inspected profile equality before removing the legacy
one-item restriction. `new` retains the v2/false-only contract. V3 ready with v1
members, v2 ready with v3 members, missing/false navigation capability, old guard
schema, changed snapshot or unmatched source bytes refuse. The navigation
boolean alone does not opt in, supply current authority or authenticate a sender.
Ordinary source-run entry still has no selected installation and refuses, even
for an empty source.

The private selected start continues to check actual current source authority,
original actor/configuration/credential binding and the same-device clock before
starting its genuine worker. All effects still require original live guard
preparation and final local admission, native durable intent/result custody and
reporting-only ACK handling. Native inBetween/nonlast and exact origin/URL checks
remain independent of the model's flow.choose route. The matching RPA runner must
arm the listener before releasing preparation, retain the original arm cutoff,
complete its maintained primitive postcheck before ACK and fence abort/UNKNOWN.
Its source pin will be paired explicitly after the consumer successor is frozen;
c32/v2 is a semantic precursor, not an accepted v3 installation.

This is target source admission, not empirical support, a publication grant, an
Android ABI/device result or successful N-1 execution. Real owned roster/parent
relations and installation inputs remain required. No source/profile qualifier
is promoted by compilation; actual matched CI/runtime evidence remains separate.


### APK reuse for test-only corrections

Normal source acceptance and the explicit llama APK/scan lane are separate.
Changes confined to Rust `cfg(test)` controls, CI selectors or documentation do
not require another APK when app/runtime/bindings/dependencies/packaging inputs
are unchanged. Retain and use the actual prior APK source/build/hash identity;
never relabel it with the corrected test-source revision.

For an explicitly reviewed reuse decision, a normal pipeline can set the
existing `TROLLM_ANDROID=0` with backend `stub`. The two stub Android jobs are
omitted while normal Rust/coverage/Node and other gates remain selected. This
is an explicit workload selection, not automatic detection of test-only source.
The independent API/web `TROLLM_ANDROID_BACKEND=llama` lane still requires its
Android build and dependency scan. No unchanged APK is rebuilt solely for a new
Git SHA, and no actual APK scan or runtime qualification is supplied by reuse.

### Private completion callback

`scripts/android/selected_native_completion.cjs` exports
`createSelectedAndroidCompletion({adb,resultDirectory})`. Its returned
`useNative(handle,ready,{signal})` composes with RPA's private installed factory.
The new result directory is absolute, private and not reused. The callback
waits for the actual launcher's inactive-worker readback before bounded reads
of that genuine run's original snapshot and event journal, then checks the same
inactive installation/claim/source binding again. Original bytes stay private;
integer tokens are inspected losslessly and never rewritten. The actual camelCase
`runId` and `eventWatermark` fields are required. For terminal evidence the
watermark must equal the final contiguous native journal sequence as an original
u64 value; an inactive interrupted prefix permits a watermark no greater than
that retained sequence and remains UNKNOWN. The Node
fixture is emitted by the actual core terminal snapshot/EventLog writer, and a
Rust control round-trips its complete shape through RunState serde. Missing, partial,
wrong-run or inconsistent evidence refuses. Nonterminal evidence requires the
passive inactive path described below and remains UNKNOWN. A failed native terminal result is
UNKNOWN. It does not settle a possible send or resend an effect.

The returned existing completion DTO contains invocationKey, status,
intentLogFile and reportFile. COMPLETED records a successful native terminal
outcome only. The separate full-cart/foreign-baseline oracle remains required;
the report explicitly leaves business outcome unestablished. This callback is
source implementation, not an actual Android run or owned-publication receipt.

### Owned install, genuine input admission and reporting

The private selected start uses the original association returned by the current
owned `MoraySource` pull/install. It rechecks `authorizeFreshStart`, reads the
exact installed `sourceBundleSetup`, and independently normalizes the original
structured input with the APK's existing `StructuredInput` implementation.
`SourceInputAdmission.attach` calls the maintained native `admitSourceInput`
with both ORIGINAL raw input and APK-normalized bytes. The same helper is used
by ordinary `RunScreen`. A conflicting supplied admission refuses rather than
being overwritten. The original raw string, not a reconstructed JSON object,
continues into `startSelectedSourceRunConfigured`; native startup recomputes and
checks the receipt and retains its original input evidence. This needs no new
launcher request field, descriptor or public support flag.

The launcher retains the genuine returned run ID before `RunHistory.record` and
`RunService.start`. Binding, cancellation and inactive-worker readback use that
same live handle. Source/input failures have closed stage classifications; a
lost start response or missing handle still does not prove worker inactivity.
The RPA callback then retains original journal/snapshot evidence through
`createSelectedAndroidCompletion`. Its `completion.json` is a private callback
result, not an API report or an independent cart predicate.

Reporting uses the existing run notification/history and `SourceRunReportPanel`:
`MoraySource.reports(...).retain(runId)` requires actual worker inactivity and
validated durable progress through `prepareSourceReport`, then retains the native progress report
and source association in the existing outbox. `send(runId)` and a later retry
send that retained report through the authenticated Moray client. They cannot
prepare or resume execution. An acknowledged report remains device-reported
progress/business-not-assessed. The independent RPA full-cart/foreign-baseline
outcome retains its own invocation/selection/native-run association; it cannot
be replaced with native success, report delivery or model prose.

The first actual join is a fresh owned original with parent `none`, followed by
separately admitted changed/subset/empty, multi-item N-1, cancellation/lost-result
UNKNOWN and reporting-only recovery controls. Genuine native parent trajectory,
checkpoint/trial continuation and cold selected reconstruction remain separate
unfinished work. Compiler `runtimeAdmissionSupported:false` describes its
materialization role and is not a new runtime switch. Any necessary APK candidate
must include the complete changed start/input/report boundary; no new device or
model execution is established by these source changes.

### Passive interrupted progress reporting

Report eligibility follows actual worker inactivity, including durable Running
or Paused snapshots after process loss. `prepareSourceReport` retains its resume
lock while reading status and source custody. It does not resume, cancel, rewrite
the phase, append a terminal event or settle a possibly sent action. Every active
worker still refuses, including a worker whose terminal phase was published
before cleanup. The existing run/history UI enables the same report controls
for inactive interrupted progress. Native preparation remains the final check;
UI eligibility alone is not proof.

The original readable matching snapshot, source association and complete bounded
journal are retained as hash-named raw files in the existing `source-reports`
directory. The snapshot watermark cannot exceed the validated contiguous journal;
the journal must remain unchanged across the observation. Existing committed
item-snapshot and input-admission checks remain mandatory. Missing, corrupt,
wrong-run or inconsistent evidence refuses new retention. The observation adds
`snapshotStatus`, `workerActive:false`, `progressDisposition` (interrupted or
terminal), `journalSha256` and `sourceAssociationSha256`. It preserves native
failure/terminal reasons. Report outcome remains unknown and business
verification remains not-assessed. Ordinary cancellation continues to report its
actual Failed/cancelled state.

`inspectTerminal` remains terminal-only. The separately named
`inspectInactiveProgress` can inspect a readable Running/Paused journal prefix
as UNKNOWN without requiring a terminal record or inventing one. It does not
prove inactivity. `createSelectedAndroidCompletion` uses it only after the same
private handle's inactive acknowledgement, and still requires the subsequent
identical inactive binding before retaining original bytes. A dead private
channel or absent binding continues to refuse; passive report support does not
implement cold selected-handle reconstruction or foreign-worker retirement.
The callback DTO and completion file remain distinct from the API outbox and
independent cart verdict. Interrupted evidence never becomes COMPLETED.

The existing retained report can be reopened and sent/retried even if current
run evidence later becomes unavailable. That delivery path never calls prepare,
resume, model or effect execution. It does not create a new report to mask an
uncertain prior delivery. The owner must retain the original outbox and receipt
bytes and join them to its separately retained independent verdict and supplier
trial/contribution identity. No business acceptance or new runtime capability
follows from passive custody.

The scoped source controls include a real killed child process with a durable
Paused source run, a separately labelled serialized Running fixture, active and
terminal-but-active refusal, real cancellation, byte-preserving repeated
retention and corrupt/missing/wrong-run/watermark/journal refusals. Node ADB/handle
controls are declared process fixtures, not installed-provider authentication.
Android UI/JVM and actual target hard-loss/report-delivery qualification remain
separate from these focused source controls.

## Android calibrated-clock source preparation

The selected emulator clock path uses the existing private guard transport, not
Android framework time confidence or a modified kernel synchronization flag.
Its host counterpart is immutable RPA source
`9c97f57a22e9b84256aecc9f6b3bf70b4bd97113`, module SHA256
`a93523bbf4db75230a44f01b923f3e6f23e8f48fe27c6bdce15c6b5ee4d853f3`.
The sample leaf is the original `sampleNow()` result. Earlier prospective
`clockVerification`/`installerClock` control projections are not this wire.

`withSelectedAndroidDriver(options, useNative, clockInstaller)` receives the
installer-held closure separately from public options. The exact private third
argument is `{sourceRevision,moduleSha256,bindAcquireClock}`. The installed RPA
composition must check its loaded source closure and supply that actual lifetime
method; the two hash strings alone do not authenticate a callback. Generic
`options.acquireClock` or `options.bindAcquireClock` refuses. Legacy calls remain
on private installation v1 with `selectedDeviceMaxErrorUs` and the real kernel
reader. They cannot silently fall back to calibration.

The calibrated caller selects `options.clockPolicy` with exactly `profileId`,
`minimumRateNumerator`, `minimumRateDenominator`, and `maxCalibrationAgeMs`.
The launcher reads the original guest boot UUID and the actual `kvm-clock`
clocksource using read-only ADB/su before provisioning. It adds the original
`guestBootId`, binds the host lifetime once, and provisions
`trollm.private-selected-installation.v2-proposal` with `clockPolicy` instead of
`selectedDeviceMaxErrorUs`. The profile discriminator is
`trollm.android-selected-emulator-calibrated-clock.v1-proposal`.
No actual rate or calibration lifetime has been selected by this source change.
The trusted operator must select the named KVM clock/lifecycle profile from its
retained launch configuration and clock observations. Numeric fields or a
profile string supplied by a model, source descriptor, or uploaded witness do
not establish that platform assumption. No snapshot restore, migration, or
external pause support is claimed. An unsupported lifecycle must refuse.

Authenticated `POST /control` accepts exactly
`{schema,invocationKey,challenge,guestBootId}`, with schema
`trollm.private-guard-clock-request.v1-proposal`. The native challenge is fresh
32-byte randomness encoded as 64 lowercase hex characters. It is consumed
before sampling and cannot be replayed. The bounded canonical request, original
cutoff, installed guest boot and invocation are checked without waiting for
worker association; startup does not deadlock on `/guard`.

The single successful response has exactly
`{schema,invocationKey,challenge,guestBootId,aborted,expiresAt,sample}`, schema
`trollm.private-guard-clock.v1-proposal`, and `sample` exactly
`{sampleUtcMs,maxErrorUs,hostBootId,originalCutoff,source,association}`.
`source` has exactly `{assuranceRef,rawSha256,probeSha256,sourcePins}`;
`association` has exactly `{owner,selectionRef,invocationKey}`. These are adapter
and installed-provider observations, not signed original kernel reply bytes.
`GET /control` remains v1 liveness only. The host sample has no future lease.

For the explicitly justified lower elapsed-clock rate `n/d`, native mapping
charges `ceil(guestNanoseconds*d/(n*1000000))` milliseconds, plus the host error
rounded upward. Both the full original request RTT and later elapsed interval
consume the original calibration-age budget. Mapping intersects the original
host/actor/publication cutoffs; existing preparation, authority, selection,
oldest-observation and navigation-arm minima are independently retained.
Recalibration does not renew any of those cutoffs. Cold restore cannot recreate
an anchor from serialized timestamps.

Calibration acquisition retains original request/response bytes plus the native
sent/received BOOTTIME samples and their original enclosing MONOTONIC endpoint
reads in exclusive private files in the allocated run
directory. Final plaintext admission uses local clock syscalls and try-locks,
with no network, credentials, or file acquisition. Original REALTIME/BOOTTIME
and BOOTTIME/MONOTONIC witness intervals are intersected rather than reset;
observed wall steps, suspend changes, regressions, source changes, or uncertain
continuity permanently fence the clock. Whole acquisition timeout, abort and
client loss fence late samples without execution or automatic retry.

Local Node transport/launcher and focused host Rust clock/authority controls are
component evidence only. `scripts/android/verify_clock_pair.cjs` additionally
consumes the original pinned RPA source with its explicit controlled C syscall
double over real private TLS; it does not qualify host synchronization. Actual
Android JNI/NDK and the selected deployment profile require separate qualification.
This preparation does not qualify guest UTC, private Android installation, live
model behavior, an effect, or a Moray report/trial. The held
MR54 merge/CI-selector action and the qualified69 APK identity remain separate.

### Clock maintenance and original run custody

The selected native worker owns a passive clock-maintenance child during model,
hold and control waits. It schedules acquisition from the remaining original
mapping-age budget, using the same installed private sampler and immutable
source/guest association. Network and raw custody use a separate acquisition
lock; a short state publication does not hold that lock across I/O. A previous
mapping remains readable only while genuinely fresh. A late replacement cannot
revive it. Acquisition timeout is bounded by the prior mapping's remaining age
and original cutoff, conservatively converted through the selected rate.
Maintenance never renews actor, publication, selection, observation, navigation
or effect authority. Current-source authorization remains a separate acquisition.

Final send remains local and nonblocking. Brief mapping publication contention
refuses a final read; cooperative model control can check again, without calling
that contention valid time or extending a deadline. Actual stale, changed,
missing or failed-source evidence permanently fences the clock. Worker cleanup
joins the owned maintenance child before publishing inactive. The original
cooperative stop cause is retained in `cancellation-cause.json` with explicit
unknown effect disposition; it does not rewrite the snapshot or claim business
settlement. Its closed codes distinguish actor expiry, stale/deadline clock,
source failure and explicit operator cancellation.

The private launcher retains immutable native input/source custody after the
original start binding, independently of later UI/report success. It also
retains snapshot, contiguous journal and every journal-referenced artifact after
actual same-worker inactive acknowledgment, before retiring the transport. This
passive cleanup uses the existing bounded cleanup interval even when the original
execution signal has expired. It never resumes, restarts or sends a report.

`selected_native_custody.cjs` preserves original embedded source strings, native
claim/private binding, original/normalized/sealed input, configuration, original
source-device preference and the full materialized manifest/member closure.
Inactive custody additionally copies actual committed artifact bytes, preserves
their original logical paths/hash/size, checks cursor membership and re-reads
snapshot/journal for drift. It does not rewrite absolute paths. Raw members and
the index are exclusive, private and fsynced. A missing/mismatched member leaves
available originals plus a closed partial index and refuses complete custody.
Launch custody explicitly does not claim worker inactivity. Neither index is a
native report, outbox, receipt, independent verdict or a replacement for missing
originals. Missing original credentials/provider state are not reconstructed.

Local declared transport and timed model controls establish component behavior,
not an Android device or real-model result. Fresh app/JNI clock reads, named KVM
lifecycle assumptions, complete workload, original native outcome and report
acceptance still require their actual qualification boundaries.
