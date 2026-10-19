# Product version and build identity

trollm has one product release version for the Rust workspace and Android app.
The root `Cargo.toml` `[workspace.package].version` is authoritative; all four
crates inherit it with `version.workspace = true`. Android uses that value as
`versionName`. Gradle reads Cargo's resolved metadata with
`cargo metadata --no-deps --offline --locked`; Android configuration needs no
Python version parser.

The current signed, installed private candidate is **0.1.0-rc.5 / code 6**,
including bundled browser v6 and the retry-wait activity fix. Source `75d74be7`
passed signed native release build and security checks. The verified APK has
SHA-256
`1d92d1caa57fb031d7b815c8fcbc9850ed6c66a877ce77c77b5176240ec2b1ce`.
An ordinary update preserved all 65 prior visible runs. The matching retry-wait
check passed Running/Pause/Resume/Cancel, cold Cancelled and service/wake cleanup,
with exactly 66 expected visible run IDs afterward. A bundled-v6 browser case
then passed: explicit count, AMBER-472, all three readings, exact title/full URL
and both live MCP calls. Full history preserved all 66 prior UUIDs plus that
run, exactly 67, with service/wake cleanup. The reasoner took 290.797 seconds
on the emulator, not phone-performance evidence. The intended tag remains
`v0.1.0-rc.5`.

Earlier rc.4/code 5 evidence remains attached to source `6deadf6` and its APK:
long-URL delivery, truncated/oversized-result refusal, imported-v6 A-B-A accuracy,
process-loss recovery, Cancel, Pause/Resume and notification checks retain their
bounded scopes. The externally imported v6 results do not substitute for the
separately verified bundled rc.5 sanity case. Actual provider login/refresh, ordinary download
transfer/recovery, Doze-specific and physical-phone behavior remain open.
Download scheduling is blocked by Android validated-network connectivity.
The private rc.4 curated source `4ef7481` and unpublished draft are preserved.
Matching rc.5 publication is pending. Source-only updates do not imply new APK
bytes or renewed acceptance.

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
