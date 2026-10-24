# trollm: pronounced “tro-le-lem”

trollm is an Android-first runtime for AI workflows that combine small local
models, built-in and MCP tools, and persistent working data. Explicit execution
plans control steps, budgets, artifacts and recovery. The name draws on TRM and
RLM, with “o” for open.

This is an **integration prototype and source-only preview**, with a Rust
workspace and Kotlin/Compose app. No ready consumer APK or supported end-to-end
quickstart is provided. Android 13/API 33+ and S21-class phones or newer are the
target; signed sideloaded APKs are the initial distribution route.

The intended experience is install → sign in → choose a supplied flow → provide
input → watch progress and results. Bundled **Summarize text**, **Browse and
summarize** and **Check a total** starters exercise the current runtime. A
connected browser may run on a PC while inference runs on the phone. Remote
tools require their services and network access; local inference does not make
every flow offline.

The app prepares checksum-pinned models and connections, supports per-flow
settings, browser OAuth and manual Bearer credentials, and shows persistent run
history. The coder calls granted built-in or MCP tools; the tool-free reasoner
supplies prose. A deterministic executor controls progress and artifacts. Models
load on first slot use after checksum verification; interrupted work requires
explicit Resume rather than restarting when the app opens.

**Check a total** uses the phone's [built-in calculator](docs/BUILTIN-TOOLS.md)
to add two amounts selected from text and compare the result with your budget,
then passes the evidence through the reasoner and back to the coder. It needs
prepared models but no MCP connection. Exact arithmetic does not prove that the
model selected the right amounts; real-model acceptance of this starter remains
to be established on an APK containing it.

## Current evidence and limits

Current source prepares **0.1.0-rc.6 / Android code 7**, adding `local.calculate`
and **Check a total**. Coverage includes scripted executor/gateway tests, bundle
checks and native tool-fence grammar tests. These checks do not establish
real-model operand selection or acceptance of an rc.6 APK.

On 2026-10-19, the signed **0.1.0-rc.5 / Android code 6** candidate completed
email-OTP OAuth login and callback, retained the connection after cold reopen,
and completed live MCP open/read plus a bounded native browser known-answer
check on an 8 GiB API 36 x86_64 emulator. The Qwen3.5-0.8B Q4_K_M coder and
Gemma 4 E2B IT Q4_K_M reasoner returned the expected answer and exact source
information. Service and wake-lock cleanup passed.

Actual OAuth refresh, ordinary download transfer/recovery, Doze-specific behavior
and physical-phone acceptance remain open. Downloads are currently blocked by
Android validated-network connectivity. Emulator checks do not establish phone
performance or arbitrary-page accuracy. A consumer APK/public release is not
ready; source updates do not renew acceptance of different APK bytes.

## Start here

- [Human flow review companion](apps/flow-review/README.md): self-hostable package
  inspection, business phases and feedback for an authoring LLM; its
  [implementation record](docs/flow-review/IMPLEMENTATION.md) tracks acceptance
- [Anatomy of a flow](docs/ANATOMY-OF-A-FLOW.md): a small current-format example
  and its scripted executor test, with no Android SDK or model weights required
- [Agent onboarding](AGENTS.md): reading order, runtime path and source map
- [Contributing](CONTRIBUTING.md): portable checks and change workflow
- [Product direction and current limits](docs/PRODUCT-PLAN.md)
- [Coder-directed flows](docs/ADAPTIVE-FLOWS.md): authored routes, control tools
  and step context, with a scripted source example
- [Android development](apps/android/README.md): build, setup and device guidance
- [Premise](docs/PREMISE.md) and [current architecture](docs/ARCHITECTURE-ANDROID.md)
- [Flow references](flows/README.md) and [execution-plan schema](schemas/execution-plan.v1.schema.json)
- [Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md) and
  [authoring guide](docs/FLOW-AUTHORING.md): proposed authoring work
- [Versioning](docs/VERSIONING.md) and [security policy](SECURITY.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md): runtime dependencies and model sources

`crates/` contains the executor, model interface, MCP gateway and UniFFI bridge.
`apps/android/` contains the app. `flows/` and `schemas/` contain current
references; scripted examples are development fixtures, and procurement is an
incomplete design sketch. The curated source tree has separate history from the
development repository; use the Issues and contribution channel for your checkout.
