# Product version and build identity

trollm has one product release version for the Rust workspace and Android app.
The root `Cargo.toml` `[workspace.package].version` is authoritative; all workspace
crates inherit it with `version.workspace = true`. Android uses that value as
`versionName`. Gradle reads Cargo's resolved metadata with
`cargo metadata --no-deps --offline --locked`; Android configuration needs no
Python version parser.

The current release is **1.0.1 / Android code 71**. It adds node-ID and artifact
filename validation to keep snapshots within their artifact directory, plus
launch documentation corrections. The original **1.0.0 / Android code 70**
release, APK and tags remain unchanged. Both include the earlier reviewed
runtime and launcher fixes, curated v342 run evidence and a native Android APK.
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

The product version selected for this patch release is `v1.0.1`; subsequent
compatible fixes advance the patch version and the next feature release is
`v1.1.0`. Use annotated tags for new releases. Never move a published release
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
