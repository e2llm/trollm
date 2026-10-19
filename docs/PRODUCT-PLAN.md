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

Browser v6 retains the explicit task bindings introduced in v5 for the selected
browser, exact URL, open action and returned tab ID. The model requests the
allowed operation without copying those values. Runtime supplies omitted
arguments and rejects conflicts before validation and mutation journaling. V6's
implemented prompt requests a direct answer, an explicit count when asked and
source title/URL. On signed rc.4 with bundled v5, native checks passed exact
527-character URL delivery and separate truncated/oversized-result refusal
before summarization, including the actual 8,192-byte serialized-result limit.
Its known-answer result contained the correct facts but omitted count/source
fields. With the same externally imported v6 bundle, pinned models and 4096-token
contexts on rc.4, a controlled amber-violet-amber sequence passed. All three live
MCP runs returned the explicit count, page code, all readings and exact source
title/full URL; the amber repeat had no violet carryover. Final enumeration
preserved exactly 60 expected visible run IDs, with no active worker or wake lock.
This is bounded known-input accuracy, not arbitrary-page accuracy or rc.5 APK
browser acceptance. These imported-flow results belong to the earlier rc.4 APK.

Source now loads models lazily on first slot use after checksum verification;
status and notifications show verification, loading and generation. Models and
run data remain in persistent app-specific storage. Lost workers leave saved
progress requiring explicit Resume. Startup never preloads models or replays a
run. Safety/review fixes, interruptible retries, durable cancellation, readable
sorted history and typed bridge errors are present. On rc.4, a tool-free run
recovered after active process loss: reopening showed Interrupted, and explicit
Resume completed that run. Exactly 58 visible run IDs remained at that checkpoint.
Other lifecycle and recovery scopes remain open.

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
their source commit and do not renew APK acceptance. The current signed,
installed candidate is **0.1.0-rc.5 / code 6**, with browser
v6 bundled in the APK. Its release build and security checks passed. The ordinary
update preserved all 65 prior visible run IDs. One new retry-wait check showed
Running during backoff and after Pause/Resume, then Cancelled after Cancel and
cold reopen, with service/wake cleanup and exactly 66 expected visible run IDs.
A bundled-v6 native browser case then passed with the correct count of three,
AMBER-472, all three readings and exact source title/full 319-character URL.
Both live MCP calls completed; service and wake lock were released. Full history
preserved exactly 67 expected visible run IDs, including all 66 prior runs.
Only the owned test tab was closed, preserving the seven baseline tabs. The
reasoner took 290.797 seconds on the emulator, not phone-performance evidence.
These bounded checks do not establish a complete consumer release.

Ordinary download transfer/recovery remains blocked by Android validated-network
connectivity. Actual provider login/refresh, Doze-specific and physical-phone
behavior, and matching source/APK publication remain open. The private rc.4
curated source and unpublished draft are preserved; matching rc.5 publication
is pending.

Earlier rc.4 candidates retain their own long-URL, page-guard, A-B-A,
process-loss, Pause/Resume, Cancel and notification results; emulator evidence
is not phone-performance evidence. These bounded checks do not establish
public-release readiness. See [versioning](VERSIONING.md). Stub/scripted checks
do not establish real-model or live-service behavior.

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
control. Native Android truncation and oversized-page refusal passed on rc.4;
the imported-v6 known-input sequence passed its required answer/source fields.
Broader page quality and matching rc.5 APK acceptance remain separate.

Source-only checkpoint publication can proceed while APK/device acceptance
remains open, with the limitations above. Broader flow-contract work, the full
authoring pack, a general catalogue and additional examples can follow the first
usable release. See [contract direction](FLOW-CONTRACT-DIRECTION.md) and
[authoring guide](FLOW-AUTHORING.md) for proposed work.
