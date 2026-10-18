# Product direction and current scope

This curated repository is a source-only prototype preview with no ready
consumer APK. Proposed interfaces and experiences remain plans until implemented
and accepted on the relevant build.

## Intended experience

A user installs trollm, signs in to E2LLM with Google or Microsoft 365, chooses a
supplied flow, provides input and follows progress and results. The browser can
run on a PC while model inference runs on the phone. Android 13/API 33+,
S21-class hardware or newer and sideloaded signed APKs are the initial target;
Google Play is outside this scope.

Defaults are text-only Qwen3.5-0.8B Q4_K_M for tools and Gemma 4 E2B IT Q4_K_M
for the final answer, using embedded non-thinking templates and 4096-token
contexts. Setup descriptors pin upstream revisions, sizes and SHA-256 hashes.

## Current source and evidence

The Rust workspace provides explicit execution plans, model slots, an MCP
gateway, durable artifacts and an Android bridge. The Compose app provides
bundle installation, verified sequential downloads, per-flow overrides,
grant-bound OAuth refresh, manual Bearer setup and bundled text/browser summary
starters. Secrets use encrypted app storage.

OAuth cleanup now handles coroutine cancellation without invalidating completed
or replacement logins. Refresh saves rotated state before exposing a token and
keeps concurrent requests coalesced through the SDK callback. Connection feedback
uses fixed failure categories without provider text or secrets. These source
checks do not establish real provider sign-in, callback or refresh acceptance.

Browser starter v5 uses explicit task bindings for the selected browser, exact
URL, open action and returned tab ID. The model requests the allowed operation
without copying those values. Runtime supplies omitted arguments and rejects
conflicts before the existing validation and mutation journal. This addresses
a native v4 failure to reproduce a long URL. On the signed rc.3 APK, a native
attempt opened the exact 527-character supplied URL, then failed in tool-call
transport before an accepted response or page reading. This does not accept the
complete browser flow or truncated/oversized-page refusal. The cause remains
under investigation; newer safe transport diagnostics are source-only and are
not in that APK.

Source now loads models lazily on first slot use after checksum verification;
status and notifications show verification, loading and generation. Models and
run data remain in persistent app-specific storage. Lost workers leave saved
progress requiring explicit Resume. Startup never preloads models or replays a
run. Safety/review fixes, interruptible retries, durable cancellation, readable
sorted history and typed bridge errors are present. The signed rc.2 candidate
passed a bounded native lifecycle check; newer source follow-ups still need
their matching APK/device checks.

Earlier APKs completed paired inference and browser v4 on an 8 GiB API 36
x86_64 emulator. With manual Bearer authentication, Qwen opened example.org once
and read the exact returned tab; tool-free Gemma summarized the complete page
accurately. Native Pause/Resume, Cancel, background/return and debug process-loss
recovery passed within their recorded scope. A signed APK (`0.1.0 / code 2`)
installed by ordinary update, preserved history and completed a new paired run,
leaving 46 runs. Download queue persistence, Cancel/Retry and free-space preflight
also passed. Ordinary unattended downloads needed operator intervention; these
checks do not establish byte-transfer recovery or real OAuth login/refresh.
The older APK is staged in a private `v0.1.0-rc.1` draft.

The signed **0.1.0-rc.2 / code 3** candidate installed through an ordinary update
and preserved all 46 prior runs and both Ready models. A new tool-free native
run passed Pause/Resume, background/return and explicit recovery after process
loss, then completed two sequential coder tasks. Cold-open showed Interrupted
without restarting work; Resume verified the model again. Service and wake lock
were released after completion, leaving 47 runs. This is an emulator result,
not physical-phone, real provider sign-in or download-transfer acceptance.
The candidate has not been published as a release. Source-only checkpoints use
their source commit and do not renew APK acceptance. The last signed, installed
candidate is **0.1.0-rc.3 / code 4**. Its build and signing
checks passed, and an ordinary emulator update preserved all 49 prior visible
runs. After the failed browser attempt, cold reopen restored the same run without
active work; full history contained exactly the prior 49 UUIDs plus that run.
The owned fixture tab was closed and the prior browser tabs preserved. A separate
tool-free rc.3 run completed one Qwen task and one Gemma task with a coherent
summary. Active background/return, terminal service/wake cleanup and completed-run
cold reopen passed; full history preserved the prior 50 UUIDs plus this run.
This does not test Doze-specific suspension or active process loss. The next
configured candidate is **0.1.0-rc.4 / code 5**, including transport diagnostics;
its build, signing and device checks remain pending. Real
provider login/refresh, ordinary download transfer/recovery, native page guards
and physical-phone acceptance remain open. Curated source is verified privately;
an unpublished rc.3 draft holds the matching verified APK and checksum. See
[versioning](VERSIONING.md). Stub/scripted checks do not establish real-model or
live-service behavior.

The [architecture](ARCHITECTURE-ANDROID.md) and
[v1 schema](../schemas/execution-plan.v1.schema.json) describe current contracts
and implementation qualifications. A broader shared flow interface and full
authoring pack remain proposed work. Minimal summary and ticket triage are also
scripted references; procurement is a schema-only sketch. A general bundle
packing command and ready-to-run host demo remain absent.

## Decisions to preserve

- Explicit flow control, small specialist models and durable artifacts drive
  execution. Keep reasoner prose, actual tool results and verified completion
  distinct.
- Flows declare model/MCP requirements and defaults, with UI overrides. Report
  actual failures clearly; normal runs need no compatibility probes or approval
  gates.
- E2LLM OAuth is the intended onboarding route; manual Bearer setup supports
  other connections. Secrets stay outside bundles.
- Jointly author a versioned flow interface with Moray contributors. Generic
  flows have no Moray runtime/account dependency. An LLM should author a small
  flow from the specification, actual tools and a requirement; recordings are
  optional provenance.
- Remote tools require services and network access. Local inference does not
  promise offline flows or keep remote-service data on the phone.
- Keep the Rust workspace and Android app together.

## Next delivery work

Accept ordinary download transfer/recovery and real provider login/refresh on
an updated combined native APK. Repeat relevant lifecycle, safety and default
starter checks on that build, then complete an S21-class arm64 pass and signed
APK delivery. Historical emulator passes do not establish these results.
The browser starter binds reading to the opened tab and rejects empty, truncated
or oversized pages; the tool-free reasoner consumes its exact artifact. A bounded
live server regression confirmed truncation reporting and a scoped complete
control, but native Android oversized-page rejection still needs acceptance.

Source-only checkpoint publication can proceed while APK/device acceptance
remains open, with the limitations above. Broader flow-contract work, the full
authoring pack, a general catalogue and additional examples can follow the first
usable release. See [contract direction](FLOW-CONTRACT-DIRECTION.md) and
[authoring guide](FLOW-AUTHORING.md) for proposed work.
