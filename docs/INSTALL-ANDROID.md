# Install trollm on Android

Download [the 1.0.1 APK](https://github.com/e2llm/trollm/releases/download/v1.0.1/app-release.apk)
from [GitHub Releases](https://github.com/e2llm/trollm/releases/tag/v1.0.1)
and open `app-release.apk` on an Android 13/API 33 or newer device. Allow your
browser or file manager to install that APK when Android asks. The release
includes a SHA-256 checksum, build identity and third-party notices alongside
the APK. Model weights are downloaded separately.

trollm is an experimental app for authored workflows with small local language
models. The [two v342 phone runs](../examples/run-342/README.md) show examples
worth inspecting; they are not a reliability or performance promise for this
release or other inputs.

## Verify the download

Download `app-release.apk.sha256` from the same release and put it beside
`app-release.apk`. On a computer with `sha256sum`, run in that directory:

```sh
sha256sum -c app-release.apk.sha256
```

It must report `app-release.apk: OK`. `SHA256SUMS` covers the other release
assets too. The release's `build-info.json` records the APK hash, source commit,
version and signing certificate under `certificateSha256`.

With Android SDK Build Tools installed, inspect the APK signature with:

```sh
apksigner verify --verbose --print-certs app-release.apk
```

The signer certificate SHA-256 must match the maintained release certificate:

```text
e34115258267beec63ff83eaf2b8cb51767a3f3b71f42cfcd3570c99ce2dc7a8
```

The checksum checks the downloaded bytes; comparing the signing certificate
checks continuity with this release identity.

## First run

1. Open trollm and choose **Summarize text** or **Check a total**.
2. Open the flow's preparation screen and download its required models. Leave
   enough device storage for the displayed files and wait for verification.
   Downloads use an unmetered network by default; the screen offers a metered
   network choice. Model preparation starts after an explicit Start or Resume.
3. Supply the flow's input and tap **Start**. Check a total expects exactly two
   item amounts in the text plus a separate budget, for example “Two items cost
   39.99 and 40.10” and budget `80`.
4. Follow progress and read the result. The calculator evaluates the selected
   amounts exactly; selecting the intended amounts remains model work.

These two starters need no browser or MCP account once their model files are
available. Local inference speed and available memory depend on the phone and
selected models. **Technical details** exposes the recorded events and identities.

## Browser workflows

**Browse and summarize** and the separately supplied
[Amazon v342 bundle](../examples/run-342/amazon-cart-v342.tar) use E2LLM to reach
a connected browser, which may run on another computer. Configure the browser
service and sign in through the connection setup before starting the flow.
Local inference does not make remote browser tools work offline.

Use **Install bundle** to import the v342 `.tar` file. Its source, model pins,
mission description and historical run evidence are in
[the example directory](../examples/run-342/README.md). Installing the app does
not install model weights or establish an E2LLM account/session.

## Updates and diagnostics

An ordinary Android update requires the same application ID and signing
certificate, plus a larger Android version code. This release uses the maintained
CI signing identity. The experimental APK68 used for the archived v342 runs has
a different recorded certificate and cannot be updated in place by this APK.
Do not remove an existing installation merely to bypass a signature mismatch:
that can discard its models, credentials and run data.

If a run stops, inspect its status before resuming. A browser change may already
have happened even if its confirmation failed. Opening the app does not replay
runs. Report problems with the app version, flow identity and relevant reviewed
diagnostics, excluding credentials and account data. See [SECURITY.md](../SECURITY.md)
for private vulnerability reporting.
