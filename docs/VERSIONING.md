# Product version and build identity

trollm has one product release version for the Rust workspace and Android app.
The root `Cargo.toml` `[workspace.package].version` is authoritative; all workspace
crates inherit it with `version.workspace = true`. Android uses that value as
`versionName`. Gradle reads Cargo's resolved metadata with
`cargo metadata --no-deps --offline --locked`; Android configuration needs no
Python version parser.

The current release version is **1.1.0 / Android code 72**. It adds the mobile
library/setup/run presentation, retained Start handoff and direct execution of
fully bound ordinary tool calls. The packaged Check a total version 2 compares
its saved sum and budget without a model call. Release validation and observed
device checks are recorded separately from these implemented capabilities.

The earlier **1.0.1 / Android code 71** added node-ID and artifact filename
validation, plus launch documentation corrections. Its release, APK and tags,
and the original **1.0.0 / Android code 70** release, remain unchanged. Both
include the earlier runtime and launcher fixes, curated v342 evidence and a
native Android APK.
The historical v342 runs used APK68 / 0.1.0-rc.24; their observations remain tied
to those recorded bytes. Source-only updates do not allocate a new APK identity.
Development branches may carry later candidates;
the next distributed APK must exceed the latest distributed Android code across
branches. Keep each existing APK tied to its actual source, not the moving branch.

Use a `v`-prefixed product version for release tags. Increase the product version
when preparing a new product release candidate or release, and strictly increase
Android `versionCode` for every newly distributed APK, including pre-releases.
Keep the signing identity for ordinary Android updates. Record the source commit,
APK checksum, package/version and signing certificate with the distributed build;
a version label alone does not identify its bytes or acceptance evidence.

The product version selected for this feature release is `v1.1.0`; subsequent
compatible fixes advance the patch version. Use annotated tags for new releases. Never move a published release
tag or replace its APK with different bytes. Prepare another version instead.

To prepare a candidate, edit the two root manifest values and refresh the local
crate entries in `Cargo.lock`, without upgrading dependencies:

```sh
cargo update --offline -p trollm-core -p trollm-models -p trollm-mcp \
  -p trollm-uniffi -p trollm-flow-review
python3 scripts/release_version.py --check
```

The check requires Python 3.11+ and uses only its standard library. It rejects
invalid version values, independently versioned workspace crates and stale
lockfile versions. APK release validation uses the same identity and checks the
actual manifest before recording the build's version.

## Android release uploads

For future signed Android releases, upload three files to GitHub Releases.

- `app-release.apk` is the signed native Android build. It contains the full
  third-party notice text and manifest. See [how to extract them](INSTALL-ANDROID.md#read-the-apk-notices).
- `build-info.json` identifies the exact APK, source commit, backend, package,
  version, Android update code, supported ABIs and signer.
- `SHA256SUMS` covers the APK and build metadata. Generate it in the directory
  containing the two verified files.

```sh
sha256sum app-release.apk build-info.json > SHA256SUMS
```

GitHub also provides a ZIP and a tarball of the tagged source automatically,
so a new release can show five download entries. [GitHub release documentation](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)

In the release notes, record the APK build source commit and matching curated
source tag. Summarize the checks performed on that build, including scripted
and JVM checks, packaging and security scans where applicable. State the limits
of device, real-model and live MCP validation. Identify any applicable
approved [security exception](SECURITY-EXCEPTIONS.md) with its scope and expiry.
Keep these facts in the release notes so they remain available after CI
artifacts expire.

Link to the [checksum-covered v342 example](../examples/run-342/README.md) when
referencing that historical bundle and its evidence. Existing releases retain
their published assets. The 1.1.0 release has eight uploaded files, including
separate notices and the historical bundle, alongside the two source archives.
Build validation still compares the embedded notices with generated copies,
as described in the [third-party notice guide](../THIRD_PARTY_NOTICES.md).

## Source-only updates

A curated source-only update is identified by its source commit. Publish
documentation-only changes through the source export without bumping the
product version, allocating an Android update code or rebuilding the APK.
Device validation stays attached to the exact APK that was checked.

Flow review container builds use full source-commit tags and content digests;
publishing one does not by itself require a Rust/Android product version bump.
Its Rust server inherits the workspace version. The private browser package's
npm metadata is not an independently published product release.
Flow/schema compatibility is versioned by its own format identifiers, such as
`ledger.execution-plan.v1` and `trollm.flow-bundle.v1`. A product version change does not
change those contracts. Model setup pins upstream model revisions, sizes and
SHA-256 hashes separately; the product version is not a model revision.
