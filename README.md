# trollm: pronounced “tro-le-lem”

trollm is an experimental Android app that runs workflows with small language
models on your phone. A workflow, or **flow**, defines the steps for a task.
Choose a flow, supply its input, and follow the run through to its report.
Install the APK directly to try a flow without compiling Rust or llama.cpp.
You can adapt and reuse the open-source app and runtime.

## Run a flow on Android

1. **Install the app.** On Android 13 or later, open the APK from GitHub Releases
   and follow Android's prompts. [Download the 1.1.0 APK](https://github.com/e2llm/trollm/releases/download/v1.1.0/app-release.apk) ·
   [Installation and verification](docs/INSTALL-ANDROID.md)
2. **Choose or add a flow.** Start with **Summarize text** or **Check a total**
   in the library. Both run locally after their models are downloaded. To add
   a flow, save its `.tar` file on your phone, then tap **Manage flows → Import
   flow file** and select it. [Download the Amazon v342 bundle (.tar)](https://raw.githubusercontent.com/e2llm/trollm/main/examples/run-342/amazon-cart-v342.tar)
3. **Connect a browser if the flow uses E2LLM.** Open the flow's connection setup
   in trollm and choose **Sign in with browser**. In the browser you want the flow
   to use, install the E2LLM extension and connect it with the **same E2LLM account**.
   That browser can be on another computer. [E2LLM browser extension setup](https://e2llm.com/start/)
4. **Run it.** Enter the flow's input and download the required models shown on
   its setup screen. Once they're ready, tap **Start run**, follow progress, and
   read the report when the run finishes.

For the Amazon example, use the connected browser's name or ID, keep only the
intended Amazon tab open in that browser, and follow the example's input format.
The flow updates the cart and stops before checkout. The recorded runs used an
earlier APK and specific model files. [Amazon v342 instructions and run evidence](examples/run-342/README.md)

## Inspect a flow on a desktop

Download the public Linux/amd64 viewer image with Docker.

```sh
docker pull --platform linux/amd64 \
  ghcr.io/e2llm/trollm-flow-review@sha256:21c4efca412dd4b80aa4a2e839b9851c609f753db93c869209034465383abad2
```

Then [start the viewer with Docker](apps/flow-review/deploy/README.md#start-the-viewer-with-docker).
The container has everything needed to inspect an uploaded flow.
[GitHub Packages](https://github.com/e2llm/trollm/pkgs/container/trollm-flow-review)

Open `http://127.0.0.1:8080` on the host running the viewer. Choose **Open files /
tar** to load the same flow bundle, or **Open folder** to load flow source.
The viewer displays the flow's steps, prompts, and tool permissions. Flow
execution and local model inference happen in the Android app.

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
