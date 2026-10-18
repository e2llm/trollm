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
present. These changes still need a matching combined native APK/device check.

Earlier APKs completed native paired inference and the browser v4 example.org
flow on an 8 GiB API 36 x86_64 emulator using manual Bearer authentication.
Pause/Resume, Cancel, background/return and debug process-loss recovery passed
within their recorded scope. An older signed APK (`0.1.0 / code 2`) installed by
ordinary update, preserved history and completed a new paired run, leaving 46
runs. Queue persistence, Cancel/Retry and free-space preflight also passed;
ordinary download transfer/recovery and real OAuth provider login/refresh remain
unaccepted. That APK is held in an older private `v0.1.0-rc.1` draft.

The next configured build candidate is **0.1.0-rc.3 / code 4**; its combined
native APK and device checks are pending. The earlier signed rc.2/code 3 APK
passed bounded update and local recovery checks, recorded in the product plan.
Remaining acceptance includes ordinary download transfer/recovery, real
provider login/refresh, an updated combined native APK and an S21-class arm64
phone. Stub/scripted checks establish their own behavior, not real inference or
live-service acceptance. See [versioning](docs/VERSIONING.md).

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
