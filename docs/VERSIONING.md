# Product version and build identity

The root `Cargo.toml` defines the product version under
`[workspace.package].version` and the Android update code under
`[workspace.metadata.android].version-code`. Workspace crates inherit the
product version. Gradle reads both values through
`cargo metadata --no-deps --offline --locked` and uses them as the APK's
`versionName` and `versionCode`.

Use an annotated `v`-prefixed tag for each product release. Give new product
releases and candidates a new version. Every newly distributed APK, including
a pre-release, needs an Android update code greater than previous distributed
builds. Retain the application ID and signing certificate for in-place Android
updates. Keep published release tags and assets tied to their original builds.
[Android installation and verification](INSTALL-ANDROID.md)

To prepare a candidate, edit the two root manifest values and refresh the local
crate entries in `Cargo.lock`, without upgrading dependencies:

```sh
cargo update --offline -p trollm-core -p trollm-models -p trollm-mcp \
  -p trollm-uniffi -p trollm-flow-review -p trollm-desktop
python3 scripts/release_version.py --check
```

The check requires Python 3.11+ and uses only its standard library. It rejects
invalid version values, independently versioned workspace crates and stale
lockfile versions. APK release validation uses the same identity and checks the
actual manifest before recording the build's version.

## Source metadata update (2026-11-20)

The public Git tags for `v1.0.0`, `v1.0.1` and `v1.1.0` were refreshed
on 2026-11-20. The APKs retain their original hashes and signing certificates.
The release notes identify both source commits. Refresh the affected tags in
an older clone with the commands below. GitHub-generated source archive hashes
can differ after a tag refresh. [GitHub Releases](https://github.com/e2llm/trollm/releases)

```sh
git fetch origin +refs/tags/v1.0.0:refs/tags/v1.0.0
git fetch origin +refs/tags/v1.0.1:refs/tags/v1.0.1
git fetch origin +refs/tags/v1.1.0:refs/tags/v1.1.0
```

## Android release uploads

Publish these files for new signed Android releases.

- `app-release.apk` is the signed native Android build. It contains the full
  third-party notice text and manifest. See [how to extract them](INSTALL-ANDROID.md#read-the-apk-notices).
- `build-info.json` identifies the exact APK, source commit, backend, package,
  version, Android update code, supported ABIs and signer.
- `SHA256SUMS` covers the APK and build metadata. Generate it in the directory
  containing the two verified files.

```sh
sha256sum app-release.apk build-info.json > SHA256SUMS
```

Release notes identify the build source commit and matching curated source tag.
They record the checks performed on that build and the scope of device,
real-model and live MCP validation. Any applicable security exception includes
its scope and expiry. [Security exceptions](SECURITY-EXCEPTIONS.md)

The historical v342 runs used APK68 / 0.1.0-rc.24. Their build, model files
and results are recorded with the checksum-covered bundle.
[v342 run evidence](../examples/run-342/README.md)

## Source-only updates

Publish documentation-only updates with the existing product version and use
the Git commit to identify the update. Android build identity and device
validation stay attached to the specific APK. Published container images
use full public source-commit tags and content digests. The flow review companion
uses `:latest`. The experimental desktop publication channel is `:preview`.
[Viewer image guide](../apps/flow-review/deploy/README.md) · [Desktop preview](../apps/desktop/README.md)

Flow schemas use identifiers such as `ledger.execution-plan.v1` and
`trollm.flow-bundle.v1` for format compatibility. Model setup pins upstream model
revisions, sizes and SHA-256 hashes. Those identities are independent of the
product version.
