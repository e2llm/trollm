# trollm: pronounced “tro-le-lem”

trollm is an Android-first runtime for authored AI workflows. Small local models
perform focused interpretation and tool choices; ordinary code carries the
procedure, exact operations, saved evidence, limits and recovery. A user chooses
a flow, supplies its inputs and follows the result instead of asking a small
model to discover an entire method in one conversation.

The **coder** is the tool-capable model role and can also write prose. The
**reasoner** supplies tool-free advice. The **runtime** runs the authored graph,
validates calls and outputs, and preserves progress. A coder can choose a declared
exit with `flow.choose`; model prose cannot invent graph edges or become a branch
condition. A flow can use one or both model roles.

This is an **integration prototype and source-only preview**, with a Rust
workspace and Kotlin/Compose app. No ready consumer APK or supported end-to-end
quickstart is provided. Android 13/API 33+ and S21-class phones or newer are the
target; signed sideloaded APKs are the initial distribution route. The name draws
on TRM and RLM, with “o” for open.

The intended experience is choose a supplied flow → configure its model files and
connections → provide input → Start → inspect progress and results. A connected
browser may run on a PC while inference runs on the phone. Remote tools require
their services and network access; local inference does not make every flow offline.

## One flow in practice

The supplied **Check a total** starter selects two amounts from text, calls the
phone's calculator, compares the saved sum with the supplied budget, then passes
the original input and both results through the reasoner and into a final report.
It needs configured model files but no MCP connection. The calculator performs exact
arithmetic; the model still has to select the intended amounts.

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

Current source metadata remains **0.1.0-rc.8 / Android code 43**. Source-only
refreshes are identified by their source commit, not a new APK version; see
[versioning](docs/VERSIONING.md).

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
their own acceptance; the main source preview is not a ready consumer release.


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
