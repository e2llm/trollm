# Contributing to trollm

Use this GitHub repository's Issues for bug reports and feature ideas. Describe
the behavior, the relevant source revision and a small reproducible example.
Coordinate design and implementation changes with the maintainer; development
and authoritative CI are maintained separately in GitLab. The curated GitHub
tree has its own history.

Read the [product direction](docs/PRODUCT-PLAN.md) and
[current architecture](docs/ARCHITECTURE-ANDROID.md). The
[flow contract direction](docs/FLOW-CONTRACT-DIRECTION.md) is proposed work, not
an implemented replacement for the [v1 schema](schemas/execution-plan.v1.schema.json).

Rust is pinned to 1.90.0. From the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --no-default-features
cargo test --locked -p trollm-uniffi --features stub
```

Default and stub checks need no model weights. Native `llama` compilation is a
separate, heavier path; passing stub tests does not establish inference.

For the included schema, wrapper and Android helper checks:

```sh
python3 -m pip install -r ci/requirements.txt
python3 scripts/validate-examples.py
ci/check-wrapper.sh
python3 -m unittest discover -s scripts/android -p 'test_*.py'
```

The workspace is split across `trollm-core` (execution), `trollm-models`
(generation), `trollm-mcp` (tools/transport) and `trollm-uniffi` (Android bridge).
See [Android development](apps/android/README.md) for the Kotlin app and local
Gradle commands. Use regression checks appropriate to changed behavior and state
whether evidence came from a scripted model, real model, JVM test or device.

Native builds need substantial RAM and disk. Bound build concurrency, check
available space and keep generated libraries, bindings, APKs and model weights
outside source control. No ready consumer release is provided by this preview.
