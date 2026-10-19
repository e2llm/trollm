# Contributing to trollm

Use this repository's Issues and contribution channel for reports and changes.
Describe the problem, intended behavior, relevant source revision and validation.
Coordinate specification, architecture and product design changes with the
maintainer. A curated source checkout has separate history from the development
repository. For vulnerabilities, follow [SECURITY.md](SECURITY.md); do not post
sensitive details in public Issues.

## Start with the current scope

Read [agent onboarding](AGENTS.md), the [product plan](docs/PRODUCT-PLAN.md) and
[current architecture](docs/ARCHITECTURE-ANDROID.md). The Android app is an
integration prototype. The schemas, parsers and architecture's implementation
qualifications define current behavior. [Flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md)
and the [authoring guide](docs/FLOW-AUTHORING.md) include proposed work, not an
implemented replacement format. Follow [versioning](docs/VERSIONING.md) for the
shared Rust/Android release identity and Android update codes.

## Find the implementation

- `crates/trollm-core`: execution plans, validation, execution and durable state.
- `crates/trollm-models`: model slots, scripted behavior and optional llama backend.
- `crates/trollm-mcp`: MCP gateway, transport, policy and cache.
- `crates/trollm-uniffi`: Android bridge and bundle handling.
- `apps/android`: Kotlin/Compose application and JVM tests; see its
  [development guide](apps/android/README.md).
- `flows`, `schemas`, `scripts`: reference inputs, contracts and portable checks.

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
