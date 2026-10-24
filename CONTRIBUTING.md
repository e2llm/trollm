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
  a planned feature is not necessarily ready to implement.
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

## Start with the current scope

Read [agent onboarding](AGENTS.md), the [product plan](docs/PRODUCT-PLAN.md) and
[current architecture](docs/ARCHITECTURE-ANDROID.md). The Android app is an
integration prototype. The schemas, parsers and architecture's implementation
qualifications define current behavior. [Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md)
and the [authoring guide](docs/FLOW-AUTHORING.md) include proposed work, not an
implemented replacement format. Follow [versioning](docs/VERSIONING.md) for the
shared Rust/Android release identity and Android update codes.

For a small end-to-end source example, follow [Anatomy of a
flow](docs/ANATOMY-OF-A-FLOW.md). It uses the current format and an existing
scripted executor test before introducing MCP-backed flows. Check
[third-party notices](THIRD_PARTY_NOTICES.md) when adding or changing dependencies.

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
scripts/check-wrapper.sh
python3 -m unittest discover -s scripts/android -p 'test_*.py'
```

See the [Android guide](apps/android/README.md) for local Gradle commands. Keep
weights, generated libraries, bindings, APKs, credentials and unsanitized reports
out of source control. Use the configured ADB connection and an explicit device
serial; coordinate one owner for device actions.

Report whether evidence came from scripted/stub dependencies, JVM tests, a real
model or live MCP. Host checks do not prove inference, authenticated service
behavior or device acceptance. Record the exact build identity for device results;
a source-only update does not establish acceptance of a new APK.
