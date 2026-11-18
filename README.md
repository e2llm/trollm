# trollm: pronounced “tro-le-lem”

trollm lets you experiment with small local language models on your Android
phone. Summarize a passage, or give it two item prices and a budget to check
with the phone's calculator. Choose a supplied flow, enter your input and follow
its progress.

[Download the 1.1.0 APK](https://github.com/e2llm/trollm/releases/download/v1.1.0/app-release.apk)
for **Android 13/API 33+** and follow the
[installation guide](docs/INSTALL-ANDROID.md). No Rust or llama.cpp build is
needed to try the app. Models are downloaded separately; **Summarize text** and
**Check a total** need no browser or MCP account once their models are available.
The app is **experimental**.

A **flow** is an authored procedure: its author supplies the steps, prompts and
allowed tools. Models interpret input, choose permitted tools and write responses.
Ordinary code runs the steps, performs exact operations, saves evidence and enforces
limits. A flow can use one or both model roles: a tool-capable **coder** and a
tool-free **reasoner**. The calculator's arithmetic is exact; selecting the intended
prices remains model work.

For other tasks, [adapt or author flows](docs/FLOW-AUTHORING.md) using local tools
or remote services through MCP (Model Context Protocol). Browser flows require
a separate connected browser and service; the browser can run on another computer.
The Android app and Rust runtime are open source: adapt the flows, change the app,
or reuse the runtime as a foundation for your own applications.

## One flow in practice

The supplied **Check a total** starter selects two amounts from text, calls the
phone's calculator, directly compares the saved sum with the supplied budget, then passes
the original input and both results through the reasoner and into a final report.
It needs configured model files but no MCP connection. The calculator performs exact
arithmetic; the model still has to select the intended amounts. The version 2
comparison step uses the bound values without another model call.

[Learn trollm through this flow](docs/LEARN-TROLLM.md) to see the complete
input → model/tool work → artifact binding → result journey. It explains why a
successful runtime check is different from a correct semantic judgment and how
Trollm relates to optional browser and authoring services.

## Learn, author and contribute

1. [Learn trollm](docs/LEARN-TROLLM.md) gives a complete product and system picture.
2. [Anatomy of a flow](docs/ANATOMY-OF-A-FLOW.md) follows a smaller, tool-free source
   example through its plan, registry, prompt and existing scripted test.
3. [Authoring a flow](docs/FLOW-AUTHORING.md) starts from intent and task contracts,
   then connects source construction, model evaluation and human review. Its wider
   proposed authoring pack is identified separately from today's method.

The [documentation tracks](docs/README.md) give ordered prerequisites, cumulative
reading costs and stopping outcomes. Use the reference map there for specific
runtime, Android, reliability or security work. Before contributing, read
[AGENTS.md](AGENTS.md) and [Contributing](CONTRIBUTING.md) for the affected layer.

## Current evidence and limits

Current release metadata is **1.1.0 / Android code 72**; see
[versioning](docs/VERSIONING.md) for source, bundle and APK identities.
The library leads with a local starter or a saved active/interrupted run. Input,
model requirements and connection setup share a flow journey, and completed runs
lead with their report. [Direct tool execution](docs/DIRECT-TOOL-EXECUTION.md)
keeps fully bound ordinary calls within the same checked dispatch and journal.

[Two recorded v342 runs](examples/run-342/README.md) show an Amazon cart workflow
on Samsung A04e and S21 phones, using local Qwen 0.8B and 2B models. Both completed
the supplied mission; their largest recorded model inputs were 1,237 and 1,253
tokens despite execution lasting about 4h20m and 1h41m. The directory contains
the exact flow, model pins, results, measurements and already-sanitized historical
logs. These are selected examples worth examining further, not a reliability
measurement. They used an earlier APK, whose identity is recorded separately
from the 1.1.0 release.

The runtime now supports prepared worklists, reusable child returns,
invocation-owned iteration and authored exits on child outcomes. Built-in tools
filter, select and order original records as well as perform exact arithmetic.
Bindings retain the evidence needed by later tasks. The
[reliability guide](docs/FLOW-RELIABILITY.md) includes a read-only selection replay
helper; [flow prompting](docs/FLOW-PROMPTING.md) describes how to test wording with
each execution model. Host/scripted checks cover these contracts. They do not
establish real-model selection accuracy or acceptance of a current-source APK.

On 2026-10-19, the signed **0.1.0-rc.5 / Android code 6** candidate completed
email-OTP OAuth login and callback, retained the connection after cold reopen,
and completed live MCP open/read plus a bounded native browser known-answer
check on an 8 GiB API 36 x86_64 emulator. The Qwen3.5-0.8B Q4_K_M coder and
Gemma 4 E2B IT Q4_K_M reasoner returned the expected answer and exact source
information. Service and wake-lock cleanup passed.

Acceptance of actual OAuth refresh, ordinary download transfer/recovery,
Doze-specific behavior and physical-phone operation remains open for the current
main build. The earlier emulator download attempt had a validated-network
connectivity failure; that is a historical test limitation, not a current claim
that downloads fail on every device. Emulator checks do not establish phone
performance or arbitrary-page accuracy. Experimental flows and other APKs require
their own checks. The downloadable 1.1.0 APK remains experimental; the historical
records describe their exact tested builds and inputs.


## Repository and contribution boundaries

`crates/` contains the executor, model interface, MCP gateway and UniFFI bridge.
`apps/android/` contains the app. `flows/` and `schemas/` contain current source
references; scripted examples are development fixtures, and procurement is an
incomplete design sketch. The separate [flow review companion](apps/flow-review/README.md)
inspects packages and collects feedback; it does not execute flows.

Trollm remains usable with direct authors and other producers. The optional
[Moray source connector](docs/MORAY-SOURCE-CONNECTOR.md) does not make a Moray
account or central controller a prerequisite for unrelated flows. Each native
contract and its current acceptance scope still applies.

The curated source tree has separate history from the development repository.
Use the Issues and contribution channel for your checkout, as described in
[Contributing](CONTRIBUTING.md). See [Versioning](docs/VERSIONING.md),
[Security](SECURITY.md) and [Third-party notices](THIRD_PARTY_NOTICES.md) for their
respective release and reporting boundaries.
