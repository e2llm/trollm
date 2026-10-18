# trollm Android development

The Kotlin/Compose app uses the Rust runtime through UniFFI. This is a developer
prototype targeting Android 13/API 33+, with S21-class hardware or newer as the
phone target. No ready consumer APK is provided here.

The included app has flow installation, run history, foreground run ownership
and execution controls. Source includes verified model downloads and a browser
OAuth client with grant-bound refresh; manual Bearer setup remains supported.
Tokens and OAuth state use Keystore-backed encrypted storage by MCP server name.
The source includes advanced per-flow overrides and bundled **Summarize text**
and **Browse and summarize** starters, installed idempotently before flow listing. Flows
appear above retained history, using their setup titles when available.
Ordinary unattended download admission and live browser OAuth/refresh still need
device acceptance; this is not a ready consumer journey.

New and retried model downloads wait for an unmetered network by default;
metered use requires a choice on the preparation screen, and roaming is blocked.
**Download model** selects one model. **Download remaining models** explicitly
queues the remaining distinct assets and shows their combined size. The active
Android transfer survives leaving the screen; admitting the rest of a batch
requires that screen to remain open. Old active transfers retain their original
policy and display Cancel/Retry guidance. Verified models are preserved.

The browser starter asks for a browser name/ID, a page URL and an optional
question. Runtime argument checks keep calls on that browser and bind the open
action to the supplied URL. The runtime supplies the exact newly opened tab ID
to reading; it rejects a conflicting model ID before dispatch and does not list
other tabs. Qwen opens and reads the page; Gemma receives its
exact artifact and has no tools. Empty, truncated or oversized results fail.

Edit starters under `flows/starters/`, update their setup pins if needed, and run
`python3 scripts/android/make_starter_bundles.py` from the repository root. The
Python helper tests verify that both committed assets match their sources.
Setup pins Hugging Face revision URLs, sizes and SHA-256 hashes for the GGUF
weights; no vision/audio projectors are downloaded.

Earlier APKs passed native text-summary, paired Qwen/Gemma inference and browser
v4 with manual Bearer authentication on an 8 GiB API 36 x86_64 emulator. The
browser opened example.org once, read the exact returned tab and gave a complete
page artifact to tool-free Gemma for an accurate visible summary. Native controls,
background/return and debug process-loss recovery also passed within their scope.

An older signed APK (`0.1.0 / code 2`) passed an ordinary emulator update,
preserved prior history and completed a new native pair, leaving 46 runs.
Queued-download persistence, Cancel/Retry and free-space preflight passed with
small fixtures. This does not establish byte-transfer recovery. That APK remains
in a private `v0.1.0-rc.1` draft; its evidence does not cover newer source.

Current source loads native slots on first use, with streaming SHA-256 verification
before construction and reuse within the run. UI/notifications show verification,
loading and generation. Persistent interrupted progress needs explicit Resume
after worker loss; startup never preloads models or replays a run. Safety/review
fixes, interruptible retry delays, durable cancellation, readable sorted history
and typed bridge errors are also present. These changes need matching combined
native APK/device acceptance.

The next configured candidate is **0.1.0-rc.4 / code 5**, with build, signing
and device checks pending. The signed, installed rc.3/code 4 APK from `dc7a331`
remains in an unpublished private draft. It preserved all 49 prior visible runs;
browser v5 opened the exact supplied URL, but its tool-call response failed
before page reading. The new transport diagnostics are not in that APK.
The earlier rc.2/code 3 APK retains its bounded update/recovery results.
Ordinary download transfer/recovery, real OAuth provider login/refresh, the
updated combined native APK and S21-class arm64 acceptance remain open. This is not a published or
accepted consumer release. See [versioning](../../docs/VERSIONING.md).

## Local prerequisites

Use JDK 17, Rust 1.90.0, Android SDK Platform 35 and NDK **27.2.12479018**.
The project pins Gradle 8.11.1, AGP 8.8.2 and Kotlin 2.1.20. Set `ANDROID_HOME`
to your SDK and `ANDROID_NDK_HOME` to that NDK directory; accept SDK licenses.
Native llama builds also need CMake, a C++ toolchain and libclang.
The version and APK-validation helpers require Python 3.11+.

From the repository root:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install --locked cargo-ndk --version 3.5.4
```

## Local builds

From `apps/android/`, start with the deterministic stub:

```sh
./gradlew --no-daemon --max-workers=2 -Ptrollm.rustFeatures=stub \
  :app:testDebugUnitTest :app:assembleDebug :app:lintDebug
```

The native backend is the Gradle default (`llama`), selectable explicitly:

```sh
./gradlew --no-daemon --max-workers=2 -Ptrollm.rustFeatures=llama :app:assembleDebug
```

Build and test the native path with the NDK and API matching the app's Gradle
configuration, then verify it on target hardware. Stub checks do not cover
native generation or loading. Check RAM/disk before native builds and model
acquisition.

Gradle builds a host library for metadata, generates Kotlin bindings with the
workspace's bindgen shim, and cross-compiles Rust for arm64-v8a, armeabi-v7a and
x86_64. Generated bindings live in `app/build/generated/uniffi`; native outputs
live in `app/src/main/jniLibs`. The debug APK is under
`app/build/outputs/apk/debug/`. Keep generated files out of source control.
Local debug builds use the developer’s Android debug signing key.
Release builds require all four explicit signing inputs below. The legacy
`TROLLM_DEBUG_KEYSTORE_PATH` affects debug builds only. Create your own private
keystore once, outside the checkout; `keytool` prompts for its password:

```bash
keytool -genkeypair -keystore /secure/path/trollm-release.jks \
  -alias trollm-release -storetype JKS -keyalg RSA -keysize 3072 -validity 10000
export TROLLM_RELEASE_KEYSTORE_PATH=/secure/path/trollm-release.jks
export TROLLM_RELEASE_KEY_ALIAS=trollm-release
read -r -s -p 'Keystore password: ' TROLLM_RELEASE_STORE_PASSWORD; echo
export TROLLM_RELEASE_STORE_PASSWORD
export TROLLM_RELEASE_KEY_PASSWORD="$TROLLM_RELEASE_STORE_PASSWORD"
```

The example uses the same key/store password; set `TROLLM_RELEASE_KEY_PASSWORD`
separately if your keystore uses a different key password. Back up the key
securely and keep its identity for future updates. A build signed by your own
key cannot update an installation signed by another publisher. From
`apps/android/`, with these variables set:

```sh
./gradlew --no-daemon --max-workers=2 -Ptrollm.rustFeatures=llama \
  -Ptrollm.androidBuildType=release \
  :app:testReleaseUnitTest :app:assembleRelease :app:lintRelease
```

After building, clear the passwords from your shell:

```sh
unset TROLLM_RELEASE_STORE_PASSWORD TROLLM_RELEASE_KEY_PASSWORD
```

Keep signing files and credentials out of source control, logs and reports.

The non-debuggable APK is under `app/build/outputs/apk/release/`, with version
`0.1.0-rc.4` (code `5`), read from the root Cargo manifest through
`cargo metadata --no-deps --offline --locked`. A private build signed by your
own key cannot update an installation signed by another publisher. This is a
build recipe; physical-phone acceptance and public APK delivery remain unfinished.

Run only flows from authors you trust. Bundles are not publisher signed and
can request remote tool calls that read, send or change data using granted
service access. See the [security policy](../../SECURITY.md) before importing
third-party flows or sharing local logs.

## Manual runtime setup

“Install bundle” opens the Android file picker. A flow bundle is an uncompressed
ustar archive with a `trollm.flow-bundle.v1` manifest, file paths and SHA-256
hashes. Parsing and hashes protect integrity relative to that manifest; they do
not establish publisher identity. A general author-facing packing command is
not supplied; the Python smoke fixtures are test-only bundles.

The runtime stores bundles, runs and legacy model slots under app-private
`filesDir/trollm/`. A pinned setup descriptor enables model preparation.
Models transfer sequentially through DownloadManager; the app verifies their
size/SHA-256 and publishes immutable hash-named files in app-specific external
storage. Rust rehashes weights before native loading. Advanced per-flow settings
can override model identities/settings and MCP endpoints or restore the pinned defaults;
effective model paths/settings are saved per run for cold resume.

Legacy `models/<slot>.gguf` plus matching `models/<slot>.gguf.sha256` remain
supported. Manual Bearer tokens are captured per gateway. OAuth runs retain the
same grant and endpoint through refresh and cold resume; logout or a replacement
login cannot silently switch their account. Live sign-in/refresh, byte-transfer recovery,
unattended downloads and physical arm64 acceptance remain open. Earlier APKs
passed debug process-loss recovery and ordinary signed update/paired-inference
checks within the historical scope above.
The browser starter passed on the emulator with manual
Bearer setup. The tool-free Qwen/Gemma pair and advanced model overrides also
passed on an 8 GiB emulator.
Never put credentials in a flow bundle.

## Android-installed corporate CAs

Android HTTPS connections, including model downloads through DownloadManager,
trust system roots and CAs installed by the user or device administrator. This
applies to both debug and release builds. Normal certificate-chain and hostname
verification remain enabled; no corporate certificate is embedded in the APK.
This deliberately trusts all CAs in the device user's CA store.

Install an approved CA through Android Settings → Security & privacy → More
security settings → Encryption & credentials → Install a certificate → CA
certificate (labels vary by Android version). Check Trusted credentials → User,
then restart trollm. Root access and system trust-store changes are unnecessary.

The app setting does not change the separate browser's trust policy or Android's
OS connectivity validation. A queued download waiting for network validation
has not reached its TLS check yet. Rust MCP uses the separate mechanism below;
Android user-store certificates are not automatically copied into Rust.

## Optional additional MCP roots

On a build supporting optional roots, an operator may supply approved CA
certificates in app-private `files/trollm/trust/mcp-roots.pem` (maximum 128 KiB).
Missing means the default Mozilla roots. Present empty, unreadable, malformed
or oversized input fails closed. New runs/gateways read the file once; existing
runs keep their captured configuration. The additional roots apply to all MCP
endpoints in this app, with normal hostname and signature verification.

This is manual developer/admin setup with no UI, separate from the Android OS
trust store. No deployment CA is embedded in the APK, flow bundles or curated
source; synthetic roots are test fixtures. For a
supporting debug APK, specify the operator's ADB server socket and device serial:

```sh
ADB_SERVER_SOCKET="$TROLLM_ADB_SERVER_SOCKET" adb -s "$TROLLM_DEVICE_SERIAL" \
  shell run-as com.trollm.app mkdir -p files/trollm/trust
ADB_SERVER_SOCKET="$TROLLM_ADB_SERVER_SOCKET" adb -s "$TROLLM_DEVICE_SERIAL" \
  exec-in run-as com.trollm.app tee files/trollm/trust/mcp-roots.pem.tmp \
  < /path/to/operator-ca.pem > /dev/null
ADB_SERVER_SOCKET="$TROLLM_ADB_SERVER_SOCKET" adb -s "$TROLLM_DEVICE_SERIAL" \
  shell run-as com.trollm.app chmod 600 files/trollm/trust/mcp-roots.pem.tmp
ADB_SERVER_SOCKET="$TROLLM_ADB_SERVER_SOCKET" adb -s "$TROLLM_DEVICE_SERIAL" \
  shell run-as com.trollm.app mv files/trollm/trust/mcp-roots.pem.tmp \
  files/trollm/trust/mcp-roots.pem
```

The temporary file is installed atomically. Removing `mcp-roots.pem` restores
default Mozilla roots for new runs.

Use a PEM approved for your deployment, then start a new run. This setup does
not establish authenticated MCP acceptance until the actual request succeeds.

See [current architecture](../../docs/ARCHITECTURE-ANDROID.md),
[product direction](../../docs/PRODUCT-PLAN.md) and
[contributor checks](../../CONTRIBUTING.md). Emulator passes do not replace
interruption/recovery, signed-release update or physical arm64 evidence from the
actual installed APK.
