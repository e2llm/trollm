# Product direction and current scope

Status reviewed on **2026-11-18**. trollm 1.1.0 is an experimental Android app
with a [downloadable native APK](https://github.com/e2llm/trollm/releases/tag/v1.1.0)
and an [installation guide](INSTALL-ANDROID.md). Contributors can also inspect
and test the runtime and self-host the flow review companion. The
[two recorded v342 phone runs](../examples/run-342/README.md) are examples worth
investigating further, not a reliability measurement.

## Intended experience

A user installs trollm, prepares models and any connections, chooses a supplied
flow, provides input and follows progress and results. A browser may run on a PC
while inference runs on the phone. Android 13/API 33+, S21-class hardware or newer
and signed sideloaded APKs are the initial target; Google Play is outside this
scope. Remote tools require their services and network access.

The coder chooses explicitly granted tools or writes prose. An explicitly
authored direct task invokes one fully bound ordinary tool call. The tool-free
reasoner gives advice. The runtime executes the authored graph, validates tool
calls and saved outputs, and enforces budgets and recovery. It does not infer a
branch from words in a model answer. [Anatomy of a flow](ANATOMY-OF-A-FLOW.md)
walks through a small current-format example.

## What is implemented

| Area | Current source |
| --- | --- |
| Flow control | Fixed transitions, coder-selected authored exits, per-step prompt supplements and generation limits; [adaptive flows](ADAPTIVE-FLOWS.md). |
| Repeated work | Prepared worklists, invocation-owned iteration, reusable child returns and declared child-outcome exits; [reliability contracts](FLOW-RELIABILITY.md) and [collection contracts](amazon-generalization/README.md). |
| Local operations | Nine isolated tools for arithmetic, assertions, worklists, totals, decimal extraction and original-record filtering/selection/ordering; [tool contracts](BUILTIN-TOOLS.md). A coder can choose a call, or an authored [direct task](DIRECT-TOOL-EXECUTION.md) can supply its complete arguments. Structural membership and exact arithmetic do not prove semantic suitability. |
| Evidence and boundaries | Typed bindings, bounded field views, model-owned argument fields, task-specific native result destinations, mutation journals, and timing/size observations; [result artifacts](TASK-RESULT-ARTIFACTS.md) and [measurement boundaries](amazon-generalization/PERFORMANCE.md). |
| Authoring and diagnosis | Current-format examples, model-specific prompt guidance and a read-only frozen-observation replay bundle helper; [Flow prompting](FLOW-PROMPTING.md) and [replay guidance](FLOW-RELIABILITY.md#replay-one-observation-before-repeating-the-mission). |
| Android | A local starter or saved run leads the library; input and prerequisites share a flow journey, with retained Start handoff, typed progress and report-first results. Bundle installation, pinned model downloads, per-flow settings, browser OAuth/manual Bearer, encrypted credentials, foreground execution and run history remain available. |
| Human review | A separate self-hosted [flow review companion](../apps/flow-review/README.md): desktop graph and inspector, business phases, source details, anchored feedback and revision comparisons. Mapped packages are read-only; uploads are transient. |

The app bundles **Summarize text**, **Browse and summarize** and **Check a total**.
The last uses local arithmetic without an MCP connection; version 2 compares
the saved sum and budget directly, without model inference for that step. Starter setup pins
text-only Qwen3.5-0.8B Q4_K_M for the coder and, where needed, Gemma 4 E2B IT
Q4_K_M for the reasoner, with non-thinking templates and 4096-token contexts.
Specialised experimental flows are not implied to be shipped starters.

Models and run data use persistent app-specific storage. After explicit
Start/Resume, up to two task-referenced models prepare in parallel after checksum
verification and are reused within the run. More than two slots retain lazy
loading; inference remains serial. Opening the app never preloads models or
replays work. Lost workers require explicit Resume. See
[model preparation](MODEL-PREPARATION.md) and the [Android guide](../apps/android/README.md).

The companion inspects packages; it does not execute flows, call models, edit
executable sources or install packages on a phone. Its [implementation
record](flow-review/IMPLEMENTATION.md) distinguishes source checks from earlier
browser/container acceptance.

## Evidence and remaining acceptance

Current release metadata is **1.1.0 / Android code 72**. Source-only
refreshes carry a source commit identity; they do not allocate a new APK version
or establish acceptance of different APK bytes. See [versioning](VERSIONING.md).

Host unit/integration checks cover the generic runtime, local tool contracts,
bundle handling and scripted flow paths. The new replay helper has packaging
checks, not a claim of real-model accuracy. A correct selection from a frozen
observation does not establish a complete browser mission.

On 2026-10-19, signed **0.1.0-rc.5 / Android code 6** passed email-OTP OAuth
login/callback, connection persistence after cold reopen, live MCP open/read and
a bounded native known-answer browser check on an 8 GiB API 36 x86_64 emulator.
The expected facts and exact source information were returned, and service and
wake-lock cleanup passed. These results retain that build/device scope.

Acceptance of actual OAuth refresh, ordinary download transfer/recovery,
Doze-specific behavior and physical-phone operation remains open for the current
main build. A previous emulator download attempt failed Android's validated-network
check; this is historical test context, not a verified current outage or a claim
about every device. Experimental phone results must retain their own flow, model
and APK identities. Source checks do not replace these acceptance tasks.

The [architecture](ARCHITECTURE-ANDROID.md), current schemas and readers define
implemented contracts. Minimal summary and ticket triage are scripted references;
procurement is an incomplete sketch. Specialised starter/replay packers exist,
but a general packer for arbitrary existing v1 packages does not. The broader
[flow contract direction](FLOW-CONTRACT-DIRECTION.md) and
[authoring pack](FLOW-AUTHORING.md) remain proposals.

## Next work

1. **Qualify the current runtime and flows.** Check representative supported,
   unsupported and ambiguous model decisions, then complete current-build native
   and physical-phone runs. Measure model preparation, generation, tool and
   runtime time separately before optimizing slow paths. Keep observation replays
   separate from end-to-end acceptance.
2. **Finish Android acceptance.** Exercise real OAuth refresh, download
   transfer/recovery, backgrounding/Doze and report/control usability on identified
   builds. Track missing evidence separately from missing implementation.
3. **Make current-format contributions easier.** Improve diagnostics, add a
   small replay walkthrough and provide a general v1 bundle packer. This work can
   proceed now; it does not depend on redesigning authoring or distributing an APK.
4. **Maintain release provenance.** Keep each APK with its matching source,
   signing/build records and generated dependency notices. Record observations
   against the exact build and hardware used.
5. **Revisit proposed authoring interfaces separately.** A shared flow contract,
   catalogue and wider authoring pack need their own design and compatibility
   review; they are not prerequisites for improving today's flows.

No delivery dates are implied by this order. The live issues below track bounded
contributions, not every maintainer/device acceptance activity.

## Help wanted

| Task | Contribution and environment |
| --- | --- |
| [#1: Useful validator error paths](https://github.com/e2llm/trollm/issues/1) | Good first issue: Python diagnostics and malformed-input fixtures; no Android or weights. |
| [#2: Validate report selection/copying](https://github.com/e2llm/trollm/issues/2) | Selection is implemented; check Android interaction on a stub device/emulator build. |
| [#3: Validate wrapping run controls](https://github.com/e2llm/trollm/issues/3) | Wrapping is implemented; check narrow widths and large text on Android. |
| [#4: General v1 bundle packer](https://github.com/e2llm/trollm/issues/4) | Intermediate Python/Rust integration work using existing bundle contracts. |
| [#5: Small non-browser replay walkthrough](https://github.com/e2llm/trollm/issues/5) | Good first issue: labelled synthetic fixtures and documentation for the existing helper. |
| [#6: APK license/NOTICE inventory](https://github.com/e2llm/trollm/issues/6) | Packaging work against an identified APK and its build inputs; coordinate with the maintainer. |

The [live help-wanted backlog](https://github.com/e2llm/trollm/labels/help%20wanted)
is authoritative for open/closed status. Follow [Contributing](../CONTRIBUTING.md)
for the GitHub issue/PR route and checks. Agent-assisted contributions are welcome;
start with [AGENTS.md](../AGENTS.md). Keep private captures, credentials and raw
phone reports out of public contributions. Dependency and distribution changes
also need [third-party notices](../THIRD_PARTY_NOTICES.md).

## Design boundaries

Keep procedure in the graph and computation in isolated tools. Model tasks ask
the coder to choose a call; explicitly authored direct tasks invoke one fully
bound ordinary call through the same checked dispatch and journal. Selected-guard
calls retain their model path. Preserve original evidence; model prose remains
advisory. Do not add site-specific executor branches, extra graph node kinds or a
flow expression language. Observe
before retrying uncertain writes. Consequential contract changes need maintainer
review; ordinary contributions can use the existing format and tests today.
