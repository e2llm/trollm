# Product direction and current scope

trollm is an Android-first integration prototype, provided here as a source-only
preview. No ready consumer APK is supplied. Proposed interfaces remain plans
until implemented and accepted on the relevant build.

## Intended experience

A user installs trollm, signs in to E2LLM, chooses a supplied flow, provides input
and follows progress and results. A browser may run on a PC while inference runs
on the phone. Android 13/API 33+, S21-class hardware or newer and signed sideloaded
APKs are the initial target; Google Play is outside this scope.

Defaults are text-only Qwen3.5-0.8B Q4_K_M for tools and Gemma 4 E2B IT Q4_K_M
for the final answer, using embedded non-thinking templates and 4096-token
contexts. Setup descriptors pin upstream revisions, sizes and SHA-256 hashes.

## Current implementation and acceptance

The Rust workspace provides explicit execution plans, model slots, an MCP
gateway, durable artifacts and an Android bridge. The Compose app provides
bundle installation, checksum-verified sequential model preparation, per-flow
overrides, browser OAuth with grant-bound refresh, manual Bearer setup and
bundled text/browser summary starters. Secrets use encrypted app storage.

Models load on first slot use after checksum verification and are reused within
the run. Status shows verification, loading and generation. Models and run data
use persistent app-specific storage. Lost workers leave saved progress requiring
explicit Resume; startup never preloads models or replays a run. Interruptible
retries, durable cancellation, readable history and typed bridge errors are present.

The browser starter binds calls to the requested browser, exact URL and newly
opened tab. It rejects conflicting arguments before dispatch and mutation
journaling. Empty, truncated or oversized pages fail before summarization;
the tool-free reasoner receives the exact accepted artifact. Verification of
structure and artifacts does not independently verify the reasoner's prose.

On 2026-10-19, signed **0.1.0-rc.5 / Android code 6** passed email-OTP OAuth
login/callback, connection persistence after cold reopen, live MCP open/read and
a bounded native known-answer browser check on an 8 GiB API 36 x86_64 emulator.
The answer included the expected facts and exact source information; service
and wake-lock cleanup passed. Earlier bounded checks covered ordinary updates,
run controls, process-loss recovery and page-size guards within their own build
scopes. Stub/scripted tests remain distinct from real-model/live-service evidence.

Actual OAuth refresh, ordinary download transfer/recovery, Doze-specific behavior
and physical-phone acceptance remain open. Download transfer is blocked by
Android validated-network connectivity. Emulator results do not establish phone
performance or arbitrary-page accuracy. The consumer release is not ready;
source-only refreshes do not imply new APK bytes or renewed device acceptance.
See [versioning](VERSIONING.md).

The [architecture](ARCHITECTURE-ANDROID.md), [execution-plan schema](../schemas/execution-plan.v1.schema.json)
and parsers define current contracts with implementation qualifications. Minimal
summary and ticket triage are scripted references; procurement is an incomplete
sketch. A general author-facing bundle packer and ready-to-run host demo are not
provided. A broader shared flow interface and complete authoring pack remain
proposed work.

## Decisions to preserve

- Explicit flow control, small specialist models and durable artifacts drive
  execution. Keep reasoner prose, actual tool results and verified completion distinct.
- Flows declare model/MCP requirements and defaults, with UI overrides. Report
  actual failures clearly; normal runs need no compatibility probes or approval gates.
- Browser OAuth is the intended onboarding route; manual Bearer setup supports
  other connections. Secrets stay outside bundles, and resumed OAuth runs retain
  their original grant and endpoint rather than adopting replacement logins.
- A versioned shared flow interface and LLM-assisted authoring are the direction.
  Generic flows have no separate authoring-service runtime or account dependency;
  recordings are optional provenance.
- Remote tools require services and network access. Local inference does not
  promise offline flows or keep remote-service data on the phone.
- Keep the Rust workspace and Android app together.

## Next delivery work

1. Accept actual OAuth refresh and ordinary model download transfer/recovery.
2. Complete relevant native starter, lifecycle and Doze checks on the candidate,
   followed by S21-class physical arm64 acceptance.
3. Prepare matching source/APK delivery with the exact build identity and scoped
   evidence. Source refresh and repository visibility are separate from release.
4. Extend the shared flow contract, authoring pack, catalogue and examples after
   the first usable release, preserving compatibility with existing consumers.

See [contract direction](FLOW-CONTRACT-DIRECTION.md) and
[authoring guide](FLOW-AUTHORING.md) for proposed authoring work.

## Contributions welcome now

Start with [Anatomy of a flow](ANATOMY-OF-A-FLOW.md) for the implemented v1
format and [Contributing](../CONTRIBUTING.md) for the GitHub issue/PR route.
These tasks improve existing behavior without depending on the proposed format:

| Task | Starting point | Scope |
| --- | --- | --- |
| [Actionable validator errors](https://github.com/e2llm/trollm/issues/1) | Python example checker | Good first issue: identify the file and field to fix. |
| [Selectable final reports](https://github.com/e2llm/trollm/issues/2) | Android run screen | Good first issue: standard text selection and copying. |
| [Responsive run controls](https://github.com/e2llm/trollm/issues/3) | Android run screen | Keep controls usable at narrow widths and large font sizes. |
| [Current-format bundle packer](https://github.com/e2llm/trollm/issues/4) | Starter packer and bridge reader | Intermediate: package existing v1 files deterministically, without a schema migration. |

Issue status is authoritative for availability; this list is not a delivery-date
commitment. See [help wanted](https://github.com/e2llm/trollm/labels/help%20wanted)
for current opportunities. Dependency changes should also update
[third-party notices](../THIRD_PARTY_NOTICES.md).
