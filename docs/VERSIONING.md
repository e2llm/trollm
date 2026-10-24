# Product version and build identity

trollm has one product release version for the Rust workspace and Android app.
The root `Cargo.toml` `[workspace.package].version` is authoritative; all workspace
crates inherit it with `version.workspace = true`. Android uses that value as
`versionName`. Gradle reads Cargo's resolved metadata with
`cargo metadata --no-deps --offline --locked`; Android configuration needs no
Python version parser.

The current source candidate is **0.1.0-rc.6 / Android code 7**. It includes
built-in `local.calculate` and the **Check a total** starter. This version bump
prepares the next APK identity; it does not transfer the recorded rc.5 device
acceptance to a new build. Curated source publication can precede APK acceptance
and does not publish a binary release or change a previous release tag.

Use a `v`-prefixed product version for release tags. Increase the product version
when preparing a new product release candidate or release, and strictly increase
Android `versionCode` for every newly distributed APK, including pre-releases.
Keep the signing identity for ordinary Android updates. Record the source commit,
APK checksum, package/version and signing certificate with the distributed build;
a version label alone does not identify its bytes or acceptance evidence.

After acceptance, the first release is `v0.1.0`; compatible fixes advance to
`v0.1.1` and the next feature release to `v0.2.0`. Never move a published release
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

Curated source-only updates are identified by their source commit. They do not
require a release-version bump and do not imply a new APK or renewed device
acceptance. Preserve the source/build identity associated with each actual APK.
Flow review container builds use full source-commit tags and content digests;
publishing one does not by itself require a Rust/Android product version bump.
Its Rust server inherits the workspace version. The private browser package's
npm metadata is not an independently published product release.
Flow/schema compatibility is versioned by its own format identifiers, such as
`ledger.execution-plan.v1` and `trollm.flow-bundle.v1`. A product version change does not
change those contracts. Model setup pins upstream model revisions, sizes and
SHA-256 hashes separately; the product version is not a model revision.
