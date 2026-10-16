# Product direction and current scope

This document preserves the agreed product direction. Proposed work describes
an intended interface and experience; it does not mark features as implemented.
This curated repository is a source-only preview with no ready consumer APK.

## Intended experience

An ordinary user installs trollm, signs in to E2LLM with Google or Microsoft 365,
chooses a supplied flow, provides starting input and follows its progress and
results. In the browser showcase, the browser can run on a PC while model
inference runs on the phone. The app prepares assets and connections declared by
the flow. The OAuth client is present; live onboarding and this complete journey
still need integrated device acceptance.

Android 13/API 33+, S21-class hardware or newer, and sideloaded signed APKs are
the initial delivery target. Intended text-only models are **Qwen3.5-0.8B** and
**Gemma 4 E2B Q4**. The initial implementation defaults are Qwen Q4_K_M for tools
and Gemma E2B IT Q4_K_M for the final answer, using embedded non-thinking templates
and 4096-token contexts. Setup descriptors pin the candidates, but the pair
still needs an actual app run; these are not verified paired presets. Google Play is outside the initial scope.

## Current source

The Rust workspace contains explicit execution plans, model slots, an MCP
gateway, durable events/artifacts and an Android bridge. The Compose app offers
bundle installation, run history and execution controls. Model preparation uses
sequential verified downloads; manual legacy slots remain supported. The OAuth
client and grant-bound refresh are implemented alongside manual Bearer setup;
secrets remain in encrypted app storage. The current source adds advanced
per-flow model/MCP overrides and bundled text-summary and browser-summary starters
installed idempotently before listing flows above history. Integrated download/setup and
live browser sign-in/refresh remain unaccepted. Stub behavior and scripted
reference tests do not establish end-to-end real-model or live-service acceptance.

The [architecture](ARCHITECTURE-ANDROID.md) and
[execution-plan schema](../schemas/execution-plan.v1.schema.json) describe current
v1 behavior and its implementation qualifications. Plans declare model slots,
context settings and MCP names/URLs; bundle manifests list files and hashes.
The optional pinned setup descriptor supplies model sources/settings,
connection defaults, titles and optional structured input fields. Input bindings
constrain tool arguments to the selected browser and opening URL before dispatch.
A broader shared replacement
contract remains proposed work. Directly authored v1 plans
may omit recorded-source provenance.

Minimal summary and ticket triage also serve as scripted test references.
The bundled text-summary starter supplies pinned Qwen setup for native use;
its integrated device journey still needs acceptance. The browser starter has
separate open/read tasks and passes the exact page artifact to a tool-free Gemma
answer task; scripted tests verify this wiring. Its native/live-browser journey
is not yet accepted. Procurement is a schema-only sketch. A general packing
command and ready-to-run host demo remain absent.

## Decisions to preserve

- Explicit flow control, small specialist models and durable artifacts drive
  execution. Keep reasoner prose, actual tool results and verified completion
  distinct.
- Flows should declare model/MCP requirements and defaults, with UI overrides.
  Do not add compatibility probes or approval gates to the normal run path;
  report actual failures clearly.
- E2LLM OAuth is the intended onboarding route. Manual Bearer configuration
  supports other connections; secrets stay outside published bundles.
- Jointly author a versioned flow interface with Moray contributors. Moray can
  produce web flows, but generic trollm flows have no Moray runtime/account
  dependency. An LLM should be able to author a small flow from the specification,
  actual tool definitions and a requirement; browser recordings are optional.
- Remote tools require their services and network access. Local inference does
  not promise every flow is offline or keep remote-service data on the phone.
- Keep the Rust workspace and Android app together. Repository publication
  alone does not justify extracting adapters or splitting the source.

## Next delivery work

Validate model setup/downloads, advanced overrides and live OAuth/refresh on an
integrated APK using the existing input, progress, result, history and run
controls. Accept both supplied starters. The browser flow has explicit browser
selection, separate opening/read tasks and rejection of empty, truncated or
oversized results. Named complete read-result artifacts feed its tool-free reasoner;
live acceptance also requires the server to report truncation accurately.
Gemma paired execution and physical-phone acceptance remain unfinished.

Run that journey with the intended pair on the actual APK and target hardware,
then deliver the signed release APK and public source. The broader shared flow
ICD, full authoring pack, general catalogue and additional examples can follow
this first usable release. Existing curated export and separate-history
publication machinery handle source delivery.

See [contract direction](FLOW-CONTRACT-DIRECTION.md) and
[authoring guide](FLOW-AUTHORING.md) for proposed fields and authoring workflow.
GitLab remains the development and validation authority. GitHub holds curated
source with separate history and is the intended future public destination.
