# Performance observations and measurement contracts

Status: **M1/M2 IMPLEMENTED IN SOURCE; DEVICE MEASUREMENT OPEN**. The bounded
observation and numeric timing/correlation changes below have synthetic host
tests. This does not establish phone latency, CPU, battery or workflow accuracy.
Historical source observations describe baseline `239e7e6`; the baseline polling
paths below remain available as strict disk APIs but Android now uses the
separate bounded observation API.

The objective is to explain where time and context go during any authored flow,
and make observing a long run inexpensive. Metrics remain observations: they
must never authorize a tool call, certify an artifact, resolve an uncertain
mutation or replace journal-based recovery.

## What the source establishes

The recollection that response-size collection repeatedly reads all logs does
not describe the current production counters. Size events are emitted where
payloads already exist. The baseline Android observer did repeatedly scan journals. That path has
been replaced for screen/service polling; full collection scans when serving
cached pages remain.
These deserve separate work from prompt and model optimization.

Historical model/context measurements are summarized below without private run
identities or raw observations. Those trials did not measure the journal-scanning
overhead described here. Preserve their configuration and scenario qualifications
when comparing later measurements.

## Existing numerical evidence

These are earlier two-item development experiments, not a benchmark of the
new count-independent package. Durations are seconds inside model calls. The captured
input comparison kept the model/settings unchanged, but cold/warm order and
split execution prevent attributing every difference to the procedure alone.

| Historical scenario | Outer model time | Prefill time | Outcome qualification |
| --- | ---: | ---: | --- |
| v100 replay, capture A, adviser plus coder | 240.654 | 210.490 | Unsuitable selection; faster variants must still be judged for correctness. |
| Same capture, coder alone | 66.213 | 53.729 | Also unsuitable; not a demonstrated replacement for semantic advice. |
| v100 replay, capture B, adviser plus coder | 200.215 | 170.989 | Unsuitable selection. |
| Same capture, coder alone | 55.224 | 43.534 | Also unsuitable. |
| v101 live heading adviser | 220.376 | 179.899 | Together with the coder, established a plausible observed prospect, not a completed shopping goal. |
| Same live heading coder | 83.113 | 74.590 | Correct original identity selection for that prospect. |
| Same live submit-control selection | 83.196 | 71.889 | 3,463 input tokens; a separate prompt-cost example. |

The v101 run took 931.460 seconds, made 39 model calls and attempted no Add.
It stopped after a navigation error despite later evidence that navigation had
occurred; subsequent flow changes address that recovery. None of these numbers
quantifies observer I/O overhead or proves that generic item iteration is faster.
They establish that prefill was a major measured cost in these calls, separately
from semantic-selection and flow-recovery failures.

## Baseline repeated I/O established by source

| Production path | Frequency and source-derived cost |
| --- | --- |
| Run screen | [RunScreen.kt](../../apps/android/app/src/main/java/com/trollm/app/ui/RunScreen.kt#L150) calls `tailEvents`, then `pollStatus`, then delays 300 ms. Each call scans the full main journal; the cursor filters returned records, not disk reads. |
| Tail reader | [run.rs](../../crates/trollm-uniffi/src/run.rs#L1234) visits and validates every record, including the old prefix. O(B) parsing/read work per call for B journal bytes; only new returned records are retained. |
| Status reader | [lib.rs](../../crates/trollm-uniffi/src/lib.rs#L438) always calls `last_event_seq`, another full scan. Success additionally invokes the report reconstruction scan. |
| Foreground service | [RunService.kt](../../apps/android/app/src/main/java/com/trollm/app/RunService.kt#L60) independently calls `pollStatus`, then delays 1000 ms. This adds status scans while service observation is active. |
| History list | [FlowListScreen.kt](../../apps/android/app/src/main/java/com/trollm/app/ui/FlowListScreen.kt#L98) reads summaries initially and refreshes active rows every two seconds. `run_summary` reads snapshot/failure metadata, not event logs or result artifacts. |
| Cached pagination | [gateway.rs](../../crates/trollm-mcp/src/gateway.rs#L953) calls `read_page_with_size` for each selected page. [cache.rs](../../crates/trollm-mcp/src/cache.rs#L197) validates the whole JSONL collection and counts bytes in that same pass, retaining only the selected page. |

The screen therefore performs at least two full main-journal scans per successful
poll iteration. Its theoretical maximum cadence is about 3.3 iterations/second;
actual cadence includes call work and scheduling. Service observation adds its
own scan. This is O(B) work per observer iteration, not necessarily an O(B)
allocation. With a growing journal and proportionally many polls, cumulative
scan work can grow quadratically. No device latency, CPU or battery impact has
been established by this source inspection.

Selecting P pages from a B-byte cached collection similarly incurs O(P × B)
collection scanning. The scan also validates data, so it cannot be classified
as telemetry-only overhead or removed on an assumed cache-trust guarantee.

## Existing metrics and their exact meanings

Sizes below are distinct representations of related data. They are not
additive. Compact JSON bytes count JSON encoding, escaping and UTF-8, not
characters, HTTP wire bytes, memory use or tokens.

| Existing observation | Definition and limitation |
| --- | --- |
| Proxy `raw_payload`, `raw_page` | Compact JSON byte count of the canonical parsed payload before shaping. Structured MCP content is selected once when also echoed as text. This is not the original response-body byte length. |
| Proxy `raw_error_payload` | Canonical parsed received-error payload bytes and MCP method origin, with remote diagnostic text redacted. Missing/malformed/partial responses do not acquire invented sizes. |
| Proxy `complete_collection` | Compact JSON bytes of the complete collected item array, including brackets and commas. An empty array has two bytes. This differs from upstream page envelopes. |
| Proxy `selected_page` | Compact JSON bytes of the selected page payload and continuation field. Distinct from the whole collection. |
| Proxy `gateway_result` | Compact JSON bytes of the model-facing value after gateway shaping. Measured for live, cached, file-served and local results. |
| Proxy `failed_result` | Null size when no parsed result returns. A failure does not mean zero response bytes. |
| Core `gatewayResultCompactJsonUtf8Bytes` | Compact JSON bytes of the returned gateway value, independently measured in core. It overlaps the proxy gateway-result measurement. Runtime control has null payload size. |
| Core `toolResultContextCandidateUtf8Bytes` | UTF-8 bytes of the encoded continuation wrapper candidate. Cancellation or a limit can prevent its use. Tool-result completion and runtime control have zero candidate bytes. |
| `artifact_written.bytes` | Actual file bytes for a returned artifact, with its SHA-256, when core successfully reads it. Includes file formatting; not necessarily compact JSON. |
| `outputBindingSizes.artifactFileBytes` | Source artifact file bytes when verifying an artifact output binding. |
| `outputBindingSizes.boundCompactJsonUtf8Bytes` | Compact JSON bytes of the bound value after an optional artifact view. It does not establish that a later prompt used that value. `viewApplied` records view presence. |
| `model_call.tokensIn` | Native tokenized, chat-templated prompt length, including model control tokens/BOS as applicable. On partial-prefill timeout it can still describe the full prompt, not tokens processed before timeout. |
| `model_call.tokensOut` | Generated token counter from the model response. This is not a character or byte count. |
| `model_call.ms`, diagnostics `totalMs` | Outer monotonic duration of the model-slot call, including lazy wrappers and teardown. The diagnostics total is populated from this same measurement. |
| `weightVerifyMs`, `loadMs` | Model checksum-verification and loading intervals, reported once when consumed. With `preparationAsync: true`, they can overlap other work and need not fall within this call's total. Warm calls leave them null; unavailable does not mean a measured zero. |
| `preparationAsync`, `preparationWaitMs` | Async preparation marker and foreground wait within `model_call.ms`. A waiting timeout can report the wait before loading diagnostics are available. Unused prepared slots have no model-call observation. Never subtract async load intervals from the call's total to infer generation time. |
| `prepareMs` | Native setup before prefill: context creation, prompt safety checks, template rendering/tokenization, sampler/grammar setup and associated work. |
| `prefillMs` | Native prompt batch preparation/decode interval, including entered partial-prefill work. |
| `generationMs` | Native sampling/output-generation interval after prefill. Timers stop before native teardown. |
| `tool_call_result.ms` | Successful outer gateway-call duration, including gateway audit/cache/shaping work. Excludes subsequent core result counting, event append and artifact verification. |
| Event `ts` | Wall-clock event timestamp. Useful for a timeline; not a monotonic duration or a reliable additive phase timer. |

Contracts and implementation:
[size semantics](../ARCHITECTURE-ANDROID.md#L1095),
[proxy event fields](../../crates/trollm-mcp/src/events.rs#L10),
[gateway boundary counting](../../crates/trollm-mcp/src/gateway.rs#L610),
[core result accounting](../../crates/trollm-core/src/exec.rs#L1672),
[binding accounting](../../crates/trollm-core/src/exec.rs#L2188),
[model diagnostic fields](../../crates/trollm-models/src/lib.rs#L57),
[lazy loading](../../crates/trollm-uniffi/src/run.rs#L700), and
[native phases](../../crates/trollm-models/src/llama.rs#L130).

Null or absent means unknown, unentered or unmeasured according to the field's
contract. Zero means an actual zero-sized representation or a measured
sub-millisecond interval. Preserve partial failure measurements. Genuine model
errors retain unknown token counts in private failure evidence rather than
being converted into successful zero-token responses.

Prepare, prefill and generation intervals are disjoint. Outer total contains
these phases plus loading/wrappers/teardown as applicable. Do not add total to
its components. Do not label the residual as prefill: it can include cooperative
holds, wrapper activity and teardown, and rounded millisecond fields need not
partition the total exactly.

## Implemented observation and measurement contracts

`observe_run(run_id, from_seq)` returns one coherent status plus at most 128
recent events and 256 KiB of raw JSON event text, a `next_seq` cursor,
`older_events_omitted` and `has_more`. Each live worker owns a disposable
projection seeded by one strict scan before worker startup; committed main-log
appends update it only after flush. The existing independent core writer and
recovery validation remain in place. The ring retains at most 256 records and
512 KiB of raw JSON; a record exceeding the batch byte limit is omitted and
advances the cursor even when the returned batch is empty. Sequence gaps alone
are not omissions. A future cursor is preserved, as in the strict tail API.

Status, activity and projection are captured under the worker's publication
locks. Final prose uses the existing last `onVerified` source-node heuristic
and root snapshot outputs, passed through verbatim. The compact source node
survives technical-ring eviction; the complete report is not truncated to the
event budget. Reports are read once at success before terminal publication.
Observer failures do not change execution, dispatch authorization or recovery.
Callback panics are contained after committed append; poisoned views produce an
observation error. Worker teardown releases its event/report projection,
including for retained volatile-failure handles.

An inactive view is reconstructed with one strict complete journal scan and
snapshot/failure metadata reads, then cached immutably. There are at most eight
inactive views, evicted by insertion age. Explicit
`invalidate_observation(run_id)` removes an inactive view; its next observation
revalidates the complete journal. Invalidation rejects live workers; explicit
resume/reconstruction invalidates any prior inactive cache entry. Lost workers
remain inactive and require explicit Resume. Cached views deliberately do not
detect external changes to older disk bytes. `tail_events` and `poll_status`
retain their full-prefix validation, limits, errors and report reconstruction.
Screen/service observation uses the new path. Android retains at most 256
technical lines and 256 KiB of UTF-8 text, with a visible omission notice. Idle
polls do not read journals, and bounded batches drain even after terminal status.

Each executor construction creates a fresh private UUID `epoch`. A dispatch
visit increments `attempt`, including retries, repeated nodes, nested frames and
foreach boundaries. Existing events carry optional `attemptContext` with the
original plan/node, frame depth and foreach positions; these are private
correlation, not inputs to prompts or recovery. Each permitted tool call gets
`callContext: {epoch, attempt, call}` before gateway validation. `ControlledGateway`
forwards that exact context and proxy `response_size.call_context` carries it
through all size boundaries. Epochs prevent resume-construction collisions;
identical arguments, timestamps and dispatch IDs are not used to join calls.
Model calls have an ordinal within their attempt. Genuine model failures emit
numeric observations with null token counts and retain original failure behavior.
A failed gateway call emits `error{phase: tool_call}` with elapsed time and exact
context, never a synthetic `tool_call_result` that could resolve mutation intent.

After each nonterminal authoritative dispatch boundary, an optional existing-type
`node_exited{phase: attempt}` observation records monotonic inclusive attempt
nanoseconds, known user-hold and retry-wait time, and their active-wall residual.
Terminal/paused attempts attach the available measurement to their existing
`run_ended`/`paused` event, preserving that event as the last lifecycle record.
These measurements end before that event's append and terminal snapshot:
`boundarySnapshotIncluded` is false, full `attemptInclusiveNs`/`activeNs` are
null, and `attemptObservedNs`/`activeObservedNs` report only the measured prefix.
The terminal snapshot's duration is unmeasured in that attempt summary.
The interval starts before the dispatch control checkpoint; a cancellation or
deadline before entry may leave no completed attempt measurement. Interrupted
attempts have no invented duration, and no duration spans process epochs.
`boundaryReturned` means the dispatch function returned successfully, including
an authored failure/partial/pause outcome; it does not mean the flow succeeded.
`userHoldNs`, `retryWaitNs` and `activeNs` are null for controls that cannot
measure holds; no-control executions measure zero hold. Known retry waits exclude
known holds to keep those categories disjoint. Active wall time still includes
model, gateway, I/O and other synchronous waits; it is not CPU time.

Attempt observations contain successful event append count/record bytes and
serialization, write+flush and observer nanoseconds, plus snapshot attempt/write
count, successful snapshot bytes, serialization and write+rename nanoseconds.
Event record bytes exclude LF delimiters. Snapshot failure intervals are retained
when entered; successful snapshot counts/bytes exclude failed writes. Trailing
attempt telemetry excludes its own append cost and never advances the persisted
recovery watermark; terminal observations retain the original event/watermark
ordering and omit their own append and terminal-snapshot cost. A failed optional
publication does not replace the original step result; genuine journal write failure remains sticky, so later
required safety writes still fail. No event type or recovery rule was added.

Model events include actual `GenerateRequest.prompt` UTF-8 bytes.
`promptRenderNs` covers template/supplement rendering and their immediate context
assembly; `promptAssemblyNs` additionally includes prompt loading and locally
available tool-schema guidance assembly. They are reported once, on the first
model call of an attempt, and overlap each other. Later continuation prompts
have their own byte count without fabricating another initial-render duration.
Existing native diagnostic phases keep their prior meanings and unknown/zero
semantics.

Gateway `response_size.timings` reports cumulative milliseconds at each size
boundary: `elapsedMs` is inclusive dispatch time; `transportMs` covers entered
transport calls, `cacheReadMs` covers entered cache reads, `resultAdmissionMs`
cumulates entered result verification/admission calls, and `resultFinishMs`
cumulates final result processing. A validated cache miss admits the payload
before cache fill and admits it again inside final processing; the first
admission lies outside finish, while the second overlaps it. Admission and
finish therefore partially overlap and neither is a disjoint component of the
other. Do not add or subtract these cumulative scopes to derive another phase.
A pre-cache admission failure can have known admission time and null finish
time. Failure observations preserve entered phases; an absent phase is
unknown/unentered, not measured zero.

Run `python3 scripts/run_metrics.py PRIVATE_MAIN_JSONL PRIVATE_PROXY_JSONL` for
an offline numeric summary. Each supplied journal is streamed once. The helper
joins/deduplicates exact attempt/model/call identities internally, counts missing
identities, duplicate/partial measurements and incomplete attempts, and emits only fixed
numeric aggregate fields with known/unknown/zero coverage. It never exports raw
IDs, text, paths, hashes or endpoints. Different phase/size boundaries remain
separate. Gateway timing aggregates use only final `gateway_result` or
`failed_result` boundaries, excluding cumulative intermediate-page timing.
It is a disposable observer, not a strict recovery validator. No upload, cache
trust change, native-model benchmark or phone action is part of these checks.

## Accounting costs that are not journal scans

[EventLog](../../crates/trollm-core/src/events.rs#L77) retains sequence and byte
counts in memory, serializes one new event and flushes before returning. It
validates history on opening. The
[proxy writer](../../crates/trollm-uniffi/src/run.rs#L460) likewise validates its
history on opening and subsequently maintains sequence in its writer closure.
These append paths do not reread the journal for each size number.

Compact JSON counters serialize to a counting writer without allocating a
second serialized string. Counting still traverses the value. Proxy and core
both count the shaped gateway result; related boundaries can traverse related
data more than once. Optimize reuse only where ownership and exact encoding
semantics permit it.

Core reads returned artifacts to hash, account and journal them, then output
verification reads artifact bindings. Registry outputs and node overrides can
read the same file again; multiple bindings can also reference one artifact.
See [artifact accounting](../../crates/trollm-core/src/exec.rs#L1722) and
[binding verification](../../crates/trollm-core/src/exec.rs#L2166). These reads
perform evidence/verification work as well as size accounting. Reuse must be
scoped to an unchanged artifact generation and preserve original evidence.

Test-only `EventLog::read_all` uses are not production telemetry paths.
[check_native_probe.py](../../scripts/android/check_native_probe.py#L120) reads
selected run files for an explicitly invoked diagnostic;
[flow_smoke.py](../../scripts/android/flow_smoke.py#L225) rereads a fixture log
during smoke checks. Neither establishes app polling behavior or device cost.

## Missing measurements and correlation

Add explicit attempt-level monotonic wall time rather than subtracting event
timestamps and calling it precise step timing. Identify plan/frame invocation,
node attempt, loop/item occurrence, model-call ordinal and tool-call identity.
Repeated visits, retries, nested frames, holds and process loss need distinct
records. Separate active execution, user hold, retry wait and interrupted time;
do not fabricate monotonic elapsed time across process restarts.

Main tool results use `intentSeq`; proxy stages use a separate `dispatch_id`.
The gateway counter starts again on gateway construction, while proxy journal
sequence continues. Main results do not carry that dispatch ID. Argument hash,
server and tool cannot uniquely join repeated identical calls or resume epochs.
Propose a shared call/attempt identity, with explicit epoch scope or durable
uniqueness, before claiming exact per-call cross-journal aggregation. Neither
timestamp proximity nor tool-name matching is sufficient.

Missing numeric boundaries worth measuring are prompt assembly/render duration,
actual request prompt UTF-8 bytes, event serialization/flush time, snapshot
serialization/write/rename time, core artifact verification time, and gateway
transport/cache/schema/shaping phases. Prompt bytes and actual input tokens
answer different questions. Track any processed-prefill token counter separately
from full prompt tokens. Failure paths need timing without inventing a successful
tool result. Snapshot writes currently serialize the entire state
([exec.rs](../../crates/trollm-core/src/exec.rs#L697)); measure before redesigning.

## Staged work and remaining gates

M1 numeric fields/offline summaries and M2 bounded observation/reconstruction
are implemented in source as described above. Device performance measurement
remains open. M3 cache indexing/reuse is deferred: cached collection page reads
continue to scan and validate the complete collection on every request, including
replacement/incomplete generations. No new cache-trust contract is introduced.
The original sequence below remains the roadmap for later measurement and reuse.

1. **Define and measure.** Agree optional numeric field definitions and phase
   containment, then add representative synthetic observer benchmarks. Record
   bytes/records processed and retained memory alongside elapsed time. Separate
   model, gateway, persistence and observation work before optimizing any one.
2. **Add a bounded worker-owned live observation projection.** Feed it from
   committed append results and completed boundaries; publish coherent immutable
   snapshots for the screen/service. Use bounded recent-event storage and
   bounded batches, including an explicit older-events-omitted indicator.
   Cap Android technical-line retention and avoid rebuilding unlimited lists.
3. **Seed reconstruction once.** For a cold opening, perform strict validated
   journal reconstruction, seed summary/watermark state and continue from the
   live owner. Bound inactive projections and release them on eviction. A new
   cold opening revalidates; metrics remain disposable derived data.
4. **Preserve the strict disk APIs.** Keep existing journal validation, limits,
   error and recovery behavior. Add a separate live observation path for UI
   consumers; do not silently weaken `tail_events` or `poll_status` semantics.
   An incremental disk offset cannot detect arbitrary edits to an old prefix
   without revalidation. A retained cursor is not a security guarantee.
5. **Optimize boundary reuse after measurement.** Consider reusing exact byte
   counts and verified artifact bytes within one immutable generation. Preserve
   audit flush ordering and failure behavior. Gate cache scan changes on a
   separately reviewed generation/invalidation and validation contract; this
   plan assumes no new trust guarantee for cached files or page metadata.
6. **Add offline numeric summaries.** Present per-attempt wall time, model
   phases/tokens, tool/persistence time and size boundaries with explicit unknown
   counts and sample coverage. Keep overlapping totals visibly separate. New
   optional payload fields need reader/consumer tests; the main event type set
   is closed, so adding event types requires compatibility review.

## Validation and privacy gates

- Scale journals and observer count independently. After initial live seeding,
  assert idle polls do not parse old prefixes and appended records are processed
  once per projection update. Test bounded batches, retained lines and eviction.
- Compare cold reconstructed summaries with live projections across repeated
  nodes, nested frames, retries/items and process epochs. Include unknown phases,
  sub-millisecond zero, partial prefill, timeout and model/gateway failures.
- Retain strict API tests for corrupt old prefixes, partial appends, oversized
  records/logs, future cursors, truncation and replacement. Existing
  [tail tests](../../crates/trollm-uniffi/src/run.rs#L1420) explicitly validate
  the old prefix. Live projection behavior must be documented separately.
- Exercise cached-page scaling and unchanged integrity/error behavior. Count
  actual collection reads; test replacement/incomplete generations and byte
  equality before proposing any validation optimization.
- Verify no observer or metrics failure authorizes dispatch, loses mutation
  intent/result ordering or changes conservative resume behavior. Verify bounded
  projection loss can be recovered from authoritative journals.
- Keep raw prompts, model/tool content, call identities, argument hashes, paths,
  endpoints and device/run identities private. Export aggregate numeric offline
  summaries without raw identifiers or automatic uploads, consistent with
  [enterprise security](../SECURITY-ENTERPRISE.md#L43).

Scripted/stub tests establish measurement wiring and scaling properties. Native
host tests can establish phase boundaries. Representative phone measurements
are a later, separately owned gate for latency/CPU/battery claims; this plan
requires no live shopping, browser writes or device work.
