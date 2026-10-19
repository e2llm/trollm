# Working in trollm

## What this project does

trollm (pronounced “tro-le-lem”) is an Android-first runtime for explicit AI
workflows using small local models, remote MCP tools and persistent working data.
It is an integration prototype targeting Android 13/API 33+; source availability
and passing host tests do not establish a ready consumer release. See the local
README and product plan for current scope and acceptance gaps.

The Kotlin/Compose app prepares flows, models and connections, then calls Rust
through UniFFI. `trollm-core` loads and validates plans and task registries and
runs a deterministic stack machine over `task`, `foreach` and `end` nodes.
`trollm-models` supplies model slots; the coder can use granted tools, while the
`reasoner` slot is tool-free. `trollm-mcp` validates and dispatches tool
calls through its gateway and policies. The executor records events, artifacts,
budgets and recovery state. Android owns UI, credentials and run lifecycle,
including a foreground service and wake lock while working. Lost workers require
explicit Resume; opening the app does not preload models or replay runs.

Model prose is advisory output. Deterministic verification checks declared
structure and artifacts; it does not prove prose accuracy. Local inference also
does not make remote-tool workflows offline or keep their tool data on-device.

## First-session reading spine

Read these before choosing where to change behavior:

1. `README.md`; purpose, prototype status and entry points.
2. `CONTRIBUTING.md`; checks and contribution workflow for this checkout.
3. `docs/PRODUCT-PLAN.md`; priorities, agreed direction and deferred work.
4. `docs/PREMISE.md`; design invariants and rationale.
5. `docs/ARCHITECTURE-ANDROID.md`; execution contracts, especially the later
   implementation qualifications and setup additions, not just the overview.
6. `flows/README.md` and the minimal-summary plan, registry and prompt it links;
   the smallest complete source example. Then read the relevant schema, parser
   and tests for the area being changed.

For Android work also read `apps/android/README.md`. For security-sensitive work
read `SECURITY.md` and `docs/SECURITY-ENTERPRISE.md`. Use the validation/readiness
record when available for the exact scope of device evidence.

## Contracts and plans

Current contracts live together in `schemas/`, the Rust parsers/validators and
`docs/ARCHITECTURE-ANDROID.md`, with implementation qualifications in that document.
If these disagree, report the concrete conflict before changing a consequential
contract. `docs/FLOW-CONTRACT-DIRECTION.md` and `docs/FLOW-AUTHORING.md` include
proposed work; do not implement their fields as if already accepted by v1.
The existing optional pinned setup descriptor has its own
`schemas/flow-setup.v1.schema.json`; inspect the actual reader before assuming a
proposed setup feature is implemented.

Task-registry prompt files supply model instructions and bindings such as
`{{ user_input }}`. The plan's `input.promptTemplate` is reserved and inactive.
The minimal-summary and ticket-triage references are exercised with scripted
dependencies. Procurement is an incomplete design sketch, not an executable
bundle. Android installation uses a hash-pinned bundle; see the bridge's bundle
reader and the Android guide rather than inventing a packaging format.

## Find the implementation

| Area | Responsibility |
| --- | --- |
| `crates/trollm-core/` | Plan/registry parsing, validation, executor, templates, events, artifacts and durable state; integration tests in `tests/`. |
| `crates/trollm-models/` | Model-slot interface, tool-call fences, chat templates and optional llama.cpp backend. |
| `crates/trollm-mcp/` | MCP transport, gateway, tool schemas, policies and cache. |
| `crates/trollm-uniffi/` | Android-facing API, bundle/setup handling, run configuration and history. |
| `apps/android/app/src/main/` | Kotlin/Compose UI, model preparation, connections/secrets and run service; JVM tests in `../test/`. |
| `flows/` | Reference plans, task registries, prompts and bundled-starter source inputs. |
| `schemas/` | Versioned execution-plan and setup JSON schemas. |
| `scripts/` | Portable validation and device helpers; development delivery tooling may also be present. |
| `docs/` | Architecture, product direction, security, versioning and development guidance. |

Keep the Rust workspace and Android app together. Preserve existing user edits.
Do not hand-edit generated UniFFI bindings, libraries, APKs or other build outputs;
change their sources and use the documented generation path.

## Make and check changes

Specification, architecture and product design decisions belong to the main
agent. Sub-agents bring material decisions to it before implementation; the main
agent consults the user when their input is needed. Within approved design, make
routine implementation choices and finish the work. If agents are used, give them
bounded tasks and distinct file ownership. Keep coordination simple.

Check available RAM and disk before builds or downloads. Reuse installed tools,
bound concurrency and keep host checks small. On constrained hosts, leave
Android/llama.cpp compilation to the existing build environment; do not download
model weights or SDK/NDK installations. For device work use the configured ADB
connection, an explicit serial and one device-action owner. Follow the Android
guide and any device runbook available in the checkout.

Rust is pinned by `rust-toolchain.toml`. Select relevant checks from
`CONTRIBUTING.md`; the common host checks, run from the repository root, are:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --no-default-features
cargo test --locked -p trollm-uniffi --features stub
python3 scripts/release_version.py --check
python3 scripts/validate-examples.py
scripts/check-wrapper.sh
python3 -m unittest discover -s scripts/android -p 'test_*.py'
```

Python dependencies are listed in `scripts/requirements.txt`; use an existing suitable
environment when available. Default/stub tests require no model weights. The
bridge without either `stub` or `llama` compiles but cannot start a model run.
Native checks and Android commands have separate requirements in the contribution
and Android guides. Run relevant tests once changes settle. Report whether
validation used scripted/stub dependencies, JVM tests, real models or live MCP;
none substitutes for another. Let long build jobs finish without polling loops.

Preserve contract compatibility and update schema, reader, examples and consumers
together when an approved format change requires it. Follow `docs/VERSIONING.md`
for shared Rust/Android versions and increasing Android update codes. Keep changes
reviewable and report remaining checks or acceptance gaps. Follow the maintainer's
authorization for merging, deployment and publication; do not add approval steps
for actions they have already authorized.

## Checkout and security boundaries

The public source tree is curated from a development checkout and has separate
history and fewer delivery/operator files. This onboarding file applies to both.
Use the local `CONTRIBUTING.md` for the correct contribution destination. Where
export tooling is present, follow its manifest and overlays when changing exported
content. Source refreshes do not publish APKs or prove device acceptance; omitted
internal tooling is not required for portable source checks.

Keep credentials, account tokens, private endpoints, device details, model weights,
caches and unsanitized diagnostic artifacts out of source and public reports.
Use placeholder configuration in examples and the existing credential storage
paths in application code. Follow `SECURITY.md` for private vulnerability reporting.
