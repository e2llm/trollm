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

Source now loads models lazily on first slot use after checksum verification;
status and notifications show verification, loading and generation. Models and
run data remain in persistent app-specific storage. Lost workers leave saved
progress requiring explicit Resume. Startup never preloads models or replays a
run. Safety/review fixes, interruptible retries, durable cancellation, readable
sorted history and typed bridge errors are present. These changes still need a
matching combined native APK/device check.

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

The current **0.1.0-rc.2 / code 3** configuration is a candidate, not a published
or accepted release. Source-only checkpoints use their source commit and do not
renew APK acceptance; see [versioning](VERSIONING.md). Stub/scripted checks do
not establish real-model or live-service behavior.

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
