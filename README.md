# trollm — pronounced “tro-le-lem”

trollm is an Android-first runtime for AI workflows that combine small local
models, MCP tools and persistent working data. Explicit execution plans control
steps, budgets, artifacts and recovery. The name draws on TRM and RLM, with “o”
for open.

This repository is a **source-only prototype preview**. There is no ready
consumer APK or supported end-to-end quickstart here. The source includes a
Rust workspace and Kotlin/Compose app with bundle installation, run history and
execution controls, model download/verification and browser OAuth client code.
The source includes per-flow advanced overrides and bundled text-summary and
browser-summary starters, installed before listing flows above run history.
Stub execution is useful for development; it does not prove real-model inference
or live MCP behavior.

The hardware target is Android 13/API 33+, on S21-class devices or newer.
**Intended models:** Qwen3.5-0.8B and Gemma 4 E2B Q4, text-only. Pinned setup
descriptors provide model identities/settings and connection defaults; the
text-summary starter uses Qwen alone. Both pinned models completed a coherent,
tool-free handoff on an API 36 x86_64 emulator with 8 GiB RAM, and the native
text-summary starter passed. Browser v4 also completed a live example.org run
with manual Bearer authentication: one open call, reading bound to its returned
tab ID, a complete nontruncated page and an accurate visible Gemma summary.
Executor time was about 159 seconds; total time was about 3 minutes 12 seconds
including model loading. Native Pause/Resume, Cancel and background/return also
passed. Model download and verification completed after operator intervention
on the emulator's partially validated network. Ordinary unattended downloads,
OAuth provider login/refresh, setup interruption and active-process recovery, physical arm64 acceptance
and signed-release updates remain open. Deployment of an upstream
truncation-reporting fix remains unverified. Release APK delivery and public
publication are unfinished.

The intended experience is install → sign in → choose a supplied flow → provide
starting input → watch progress and results. A connected browser may run on a PC
while inference runs on the phone. Remote tools still require their services
and network access; on-device inference does not make every flow offline.

- [Product direction and current limits](docs/PRODUCT-PLAN.md)
- [Android development](apps/android/README.md)
- [Contributing](CONTRIBUTING.md)
- [Premise](docs/PREMISE.md) and [current v1 architecture](docs/ARCHITECTURE-ANDROID.md)
- [Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md) and
  [authoring guide](docs/FLOW-AUTHORING.md) — proposed authoring work
- [Security design](docs/SECURITY-ENTERPRISE.md)

`crates/` contains the executor, model interface, MCP gateway and UniFFI bridge.
`apps/android/` contains the app. `flows/` and `schemas/` contain current v1
references; examples using scripted dependencies are development fixtures.

GitLab remains the authority for development and validation. This private GitHub
repository now holds curated source with separate history and is the intended
future public destination. It provides source, without a GitHub build/release
workflow or downloadable release claims. Reports may be filed in its Issues.
