# Product version and build identity

trollm has one product release version for the Rust workspace and Android app.
The root `Cargo.toml` `[workspace.package].version` is authoritative; all four
crates inherit it with `version.workspace = true`. Android uses that value as
`versionName`. Gradle reads Cargo's resolved metadata with
`cargo metadata --no-deps --offline --locked`; Android configuration needs no
Python version parser.

The next configured candidate is **0.1.0-rc.4**, with Android **versionCode 5**
from `[workspace.metadata.android].version-code` in the root manifest. It includes
the transport-diagnostic follow-up. Its build, signing and device checks remain
pending; the planned release tag is `v0.1.0-rc.4`.

The last signed, installed candidate is **0.1.0-rc.3 / code 4**, from source
`dc7a331`. Its build and signing checks passed; an ordinary emulator update
preserved all 49 prior visible runs. A browser v5 attempt opened the exact
supplied URL, but the tool-call response failed before page reading. Full flow,
OAuth, download-transfer and physical-phone acceptance remain open. An
unpublished private `v0.1.0-rc.3` draft retains that verified APK and checksum
alongside its matching curated source.

The transport-diagnostic follow-up is not in the rc.3 APK. Source-only updates
do not imply new APK bytes or renewed device acceptance.

The previously tested **0.1.0-rc.2 / code 3** APK retains its own source identity,
bytes and acceptance evidence. Its checks do not establish acceptance of the
new combined candidate.

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
cargo update --offline -p trollm-core -p trollm-models -p trollm-mcp -p trollm-uniffi
python3 scripts/release_version.py --check
```

The check requires Python 3.11+ and uses only its standard library. It rejects
invalid version values, independently versioned workspace crates and stale
lockfile versions. APK release validation uses the same identity and checks the
actual manifest before recording the build's version.

Curated source-only updates are identified by their source commit. They do not
require a release-version bump and do not imply a new APK or renewed device
acceptance. Preserve the source/build identity associated with each actual APK.
The older private `v0.1.0-rc.1` draft contains an APK whose embedded version is
`0.1.0` / code `2`; its historical label and evidence must not be rewritten to
match the new configuration.

Flow/schema compatibility is versioned by its own format identifiers, such as
`ledger.execution-plan.v1` and `trollm.flow-bundle.v1`. A product version change does not
change those contracts. Model setup pins upstream model revisions, sizes and
SHA-256 hashes separately; the product version is not a model revision.
