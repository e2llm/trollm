# trollm — pronounced “tro-le-lem”

trollm is an Android-first runtime for AI workflows that combine small local
models, MCP tools and persistent working data. Explicit execution plans control
steps, budgets, artifacts and recovery. The name draws on TRM and RLM, with “o”
for open.

This repository is a **source-only prototype preview**, with a Rust workspace
and Kotlin/Compose app. No ready consumer APK or supported end-to-end quickstart
is provided. Android 13/API 33+ and S21-class phones or newer are the target.
The intended text-only models are Qwen3.5-0.8B Q4_K_M and Gemma 4 E2B IT Q4_K_M.

Source includes verified model preparation, advanced per-flow settings, browser
OAuth with grant-bound refresh, manual Bearer setup, and bundled **Summarize
text** and **Browse and summarize** starters. Native models load on first slot
use after checksum verification. Status shows verification/loading/generation;
lost workers require explicit Resume. Safety/review fixes, interruptible retries,
durable cancellation, readable sorted history and typed bridge errors are also
present. The installed rc.5 APK has bounded update and retry-wait acceptance;
its bundled-v6 browser sanity check also passed.

Earlier APKs completed native paired inference and the browser v4 example.org
flow on an 8 GiB API 36 x86_64 emulator using manual Bearer authentication.
Pause/Resume, Cancel, background/return and debug process-loss recovery passed
within their recorded scope. An older signed APK (`0.1.0 / code 2`) installed by
ordinary update, preserved history and completed a new paired run, leaving 46
runs. Queue persistence, Cancel/Retry and free-space preflight also passed;
ordinary download transfer/recovery and real OAuth provider login/refresh remain
unaccepted. That APK is held in an older private `v0.1.0-rc.1` draft.

The current signed, installed candidate is **0.1.0-rc.5 / code 6**, with browser
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

Earlier rc.4 checks passed exact long-URL delivery, separate truncated/oversized
refusal and explicit recovery after process loss. OAuth browser handoff and
cancellation passed without accepting account login/refresh.

Browser v6 retains v5's exact task bindings and requests a direct answer,
a count when requested and the source title/URL. A controlled amber-violet-amber
sequence passed with the same externally imported v6 bundle, pinned models and
4096-token contexts on the rc.4 executor. All three live MCP runs returned the
explicit count, page code, all readings and exact source title/full URL; the
amber repeat contained no violet carryover. Final enumeration preserved exactly
60 expected visible run IDs, with no active worker or wake lock. This bounded
known-input result does not establish arbitrary-page accuracy or accept rc.5.
Those imported-flow results belong to rc.4, separate from the completed
bundled rc.5 sanity case. Ordinary download transfer/recovery remains blocked by Android
validated-network connectivity. Actual provider login/refresh, Doze-specific
and physical-phone behavior, and matching source/APK publication remain open. The private rc.4
curated source and unpublished draft are preserved; matching rc.5 publication
is pending. The consumer release is not yet ready. See
[versioning](docs/VERSIONING.md).

The intended experience is install → sign in → choose a supplied flow → provide
starting input → watch progress and results. A connected browser may run on a PC
while inference runs on the phone. Remote tools require their services and
network access; on-device inference does not make every flow offline.

- [Product direction and current limits](docs/PRODUCT-PLAN.md)
- [Android development](apps/android/README.md)
- [Contributing](CONTRIBUTING.md)
- [Premise](docs/PREMISE.md) and [current v1 architecture](docs/ARCHITECTURE-ANDROID.md)
- [Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md) and
  [authoring guide](docs/FLOW-AUTHORING.md) — proposed authoring work
- [Security policy and private reporting](SECURITY.md)
- [Security design](docs/SECURITY-ENTERPRISE.md)

`crates/` contains the executor, model interface, MCP gateway and UniFFI bridge.
`apps/android/` contains the app. `flows/` and `schemas/` contain current v1
references; examples using scripted dependencies are development fixtures.

This curated source tree has separate history from the development repository.
Use this GitHub repository's Issues for reports and feature ideas.
