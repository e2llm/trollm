# Contributing to trollm

Use [GitHub Issues](https://github.com/e2llm/trollm/issues) for community bug
reports, questions and proposed improvements, and open pull requests against
GitHub `main`. Contributors do not need access to the development repository.
If you already work in that repository, use its existing issue/MR workflow.
For vulnerabilities, follow [SECURITY.md](SECURITY.md); do not post sensitive
details in Issues or pull requests.

Agent-assisted contributions are welcome. Start from [AGENTS.md](AGENTS.md),
review the generated changes and run the relevant checks. The contributor is
responsible for the patch and its validation regardless of the tools used.

## Choose and submit a change

- Browse [good first issues](https://github.com/e2llm/trollm/labels/good%20first%20issue)
  for bounded starting points and [help wanted](https://github.com/e2llm/trollm/labels/help%20wanted)
  for larger contributions. The [product plan](docs/PRODUCT-PLAN.md) gives context;
  a planned feature is not necessarily ready to implement. Issues marked
  `validation` ask for checks of implemented behavior; read their remaining
  acceptance criteria before proposing a replacement implementation.
- Describe the problem, intended behavior, relevant source/build revision and
  how to reproduce it. Include only sanitized examples and diagnostics.
- Discuss specification, architecture and product design changes with the
  maintainer before implementing them. Routine fixes within the current contract
  can go straight to a focused pull request.
- Explain what changed and why, link the issue if there is one, and report the
  checks actually run and their limits. Include tests for changed behavior where
  useful; documentation-only changes do not need unrelated builds.

### How a GitHub pull request reaches the source tree

GitHub contains a curated source tree with separate history from the development
repository. Maintainers review a GitHub pull request, integrate accepted changes
into the development tree while retaining contributor attribution, run its
checks, and publish the resulting curated snapshot. The maintainer links that
GitHub commit back to the pull request before closing it as integrated. A public
pull request may therefore be closed without GitHub marking it as merged.

Do not merge a GitHub-only change and assume it will survive the next export.
Maintainers must first integrate it into the source tree or its export policy.
Contributors should branch from GitHub `main`; they do not need to reproduce the
private delivery setup or work with the other repository's history.

New snapshot commits are published on behalf of maintainer
[soko246](https://github.com/soko246), using that account's GitHub-provided commit
identity. GitHub's contributor graph counts these curated publications. Original
development authorship and community contribution discussions remain separate
records; each snapshot's `Source-Commit` trailer identifies its source revision.

Source attribution metadata was updated on 2026-11-20. Source files and uploaded
release assets are unchanged; Git commit and tag identities changed. Existing
clones should preserve local work and base new changes on the updated `main`.
See the [source metadata update](docs/VERSIONING.md#source-metadata-update-2026-11-20)
for instructions to refresh existing release tags.

## Start with the current scope

Use the [reading tracks](docs/README.md) to choose preparation for your task.
[Learn trollm](docs/LEARN-TROLLM.md) supplies a complete product picture; the
[agent context](AGENTS.md) maps source and working rules. The
[product plan](docs/PRODUCT-PLAN.md) and current issues identify priorities, while
the [architecture](docs/ARCHITECTURE-ANDROID.md), schemas and readers describe
the affected runtime contract and implementation qualifications.

The Android app is experimental and distributed as signed APK releases.
[Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md)
and the proposed pack in the [authoring guide](docs/FLOW-AUTHORING.md) do not
replace the current format; the guide's task-design and testing method can be
used today. Follow [versioning](docs/VERSIONING.md) for the shared Rust/Android
identity and update codes. Documentation-only work does not require unrelated
runtime or Android builds.

For a small end-to-end source example, follow [Anatomy of a
flow](docs/ANATOMY-OF-A-FLOW.md). It uses the current format and an existing
scripted executor test before introducing MCP-backed flows. Check
[third-party notices](THIRD_PARTY_NOTICES.md) when adding or changing dependencies.

### A starting point for your environment

| Contribution | What you need |
| --- | --- |
| Documentation, example diagnostics or replay fixtures | Python and `scripts/requirements.txt`; no Android SDK or model weights |
| Executor, local tools or bundle contracts | The pinned Rust toolchain and relevant scripted/stub tests; no model weights |
| Flow review companion | Its Rust server and Node/browser tooling; no Android build |
| Android presentation or lifecycle validation | The Android build environment and an emulator or phone; state which build you checked |
| Prompt quality experiments | The intended coder/reasoner models and reviewed observations; report these separately from scripted tests |

For flow changes, use [Flow prompting](docs/FLOW-PROMPTING.md) and
[Flow reliability](docs/FLOW-RELIABILITY.md) alongside the current schemas.
Keep original evidence, reasoner advice and expected answers distinct. Test
supported choices and no-answer cases, with actual model identity, context,
thinking settings and generation bounds recorded. A scripted model verifies
wiring, not the quality of the prompt. Keep private captures and credentials out
of contributions; label synthetic examples as synthetic.

## Find the implementation

- `crates/trollm-core`: execution plans, validation, execution and durable state.
- `crates/trollm-models`: model slots, scripted behavior and optional llama backend.
- `crates/trollm-mcp`: MCP gateway, transport, policy and cache.
- `crates/trollm-uniffi`: Android bridge and bundle handling.
- `apps/android`: Kotlin/Compose application and JVM tests; see its
  [development guide](apps/android/README.md).
- `flows`, `schemas`, `scripts`: reference inputs, contracts and portable checks.

The separate [flow review companion](apps/flow-review/README.md) lives in
`apps/flow-review/` with its own browser package and a Rust workspace server.
Read its [design and interface](docs/flow-review/README.md) before changing review
semantics. Use its focused frontend/server checks for viewer-only changes;
Android/native compilation is not required for those changes.

Keep the Rust workspace and Android app together. Preserve existing user edits
and generate bindings/build outputs from source instead of hand-editing them.

## Validate a change locally

Check RAM and free disk before builds or downloads. Reuse installed tools and
bound concurrency. On constrained hosts, use the existing build environment for
Android/native compilation rather than installing SDK/NDK packages or downloading
model weights. Run relevant regression checks once changes settle.

Rust is pinned to 1.90.0 in `rust-toolchain.toml`. From the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --no-default-features
cargo test --locked -p trollm-uniffi --features stub
cargo clippy --locked -p trollm-uniffi --all-targets --features stub,bindgen-cli -- -D warnings
```

Default/stub checks need no model weights. The `stub` feature supplies
deterministic responses; a bridge without `stub` or `llama` compiles but cannot
start a model run. Native compilation is a separate, heavier check:

```sh
cargo test --locked -p trollm-models --features llama --lib
```

This exercises grammar semantics with a tiny generated vocabulary and no weights.
Two real-generation tests are explicitly ignored. To run them separately, set
`TROLLM_TEST_MODEL` to an existing instruction GGUF and add
`-- --ignored --test-threads=1`; they fail if the fixture is missing.

For schemas, versions, the Gradle wrapper and Android helpers, use Python 3.11+:

```sh
python3 -m pip install -r scripts/requirements.txt
python3 scripts/release_version.py --check
python3 scripts/validate-examples.py
python3 scripts/check-docs.py
python3 scripts/check-run-examples.py
python3 -m unittest discover -s scripts -p 'test_*.py'
scripts/check-wrapper.sh
python3 -m unittest discover -s scripts/android -p 'test_*.py'
```

`check-docs.py` checks Markdown hygiene; `check-run-examples.py` verifies the
curated run checksums, identities and measurements. Android release builds use
`scripts/android/make_apk_notices.py` to collect notices from resolved Rust,
Maven and NDK inputs. Run it through the [Android build procedure](apps/android/README.md)
so its inputs match the APK; its unit tests are included in the helper suite above.

See the [Android guide](apps/android/README.md) for local Gradle commands. Keep
weights, generated libraries, bindings, APKs, credentials and unsanitized reports
out of source control. Use the configured ADB connection and an explicit device
serial; coordinate one owner for device actions.

Report whether evidence came from scripted/stub dependencies, JVM tests, a real
model or live MCP. Host checks do not prove inference, authenticated service
behavior or device acceptance. Record the exact build identity for device results;
a source-only update does not establish acceptance of a new APK.
