# Third-party notices and dependency inventory

This prototype uses the projects credited below. Third-party software and model
weights retain their own licenses; trollm's [Apache-2.0 license](LICENSE) does
not replace them. This is a source-level attribution and a guide to the remaining
packaging work, not an exhaustive notice bundle for a distributed APK.

Versions below come from `Cargo.lock`, the Android Gradle files and the starter
setup descriptors. They describe this checkout; feature selection, target ABI,
Gradle resolution and the APK build type affect what is actually shipped.
A lock file or an SBOM does not establish a byte-reproducible build.

## Rust application libraries

These are the external direct dependencies of the original runtime crates.
Links identify the upstream projects and the versioned published source where
available. The license column records the upstream license family; retain the
actual license, copyright and any NOTICE files from the exact package when
preparing a distribution. License identifiers were checked against the cached,
locked Cargo package manifests.
Complete transitive-package notice collection is still outstanding.

The separate flow review server additionally uses the locked dependencies below.
License identifiers were read from their Cargo manifests. They do not enable
native inference or add dependencies to Android.

| Project | Locked version | Use | License |
| --- | --- | --- | --- |
| axum | 0.8.9 | Inspection HTTP API | MIT |
| Tokio | 1.53.2 | HTTP runtime and bounded work | MIT |
| tower-http | 0.6.11 | Static assets, limits and headers | MIT |
| tempfile | 3.27.0 | Transient import staging | MIT OR Apache-2.0 |
| tar | 0.4.46 | Strict upload archive preflight | MIT OR Apache-2.0 |
| rustix | 1.1.5 | Descriptor-based mapped-directory reads | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |

The flow review browser uses React and React DOM 19.3.1, React Flow
(`@xyflow/react`) 12.12.0 and Dagre (`@dagrejs/dagre`) 1.1.5, all MIT-licensed
according to the installed locked packages. Its exact dependency tree is in
`apps/flow-review/web/package-lock.json`. Vite, TypeScript, ESLint, Vitest and
Playwright are development/build/test tools. Retain upstream notices when
distributing browser assets or the container image.

| Project | Locked version | Use | Upstream license / source |
| --- | --- | --- | --- |
| anyhow | 1.0.104 | Error handling | MIT OR Apache-2.0; [source](https://docs.rs/crate/anyhow/1.0.104/source/) |
| thiserror | 2.0.21 | Error types | MIT OR Apache-2.0; [source](https://docs.rs/crate/thiserror/2.0.21/source/) |
| Serde | 1.0.229 | Serialization | MIT OR Apache-2.0; [source](https://docs.rs/crate/serde/1.0.229/source/) |
| serde_json | 1.0.152 | JSON | MIT OR Apache-2.0; [source](https://docs.rs/crate/serde_json/1.0.152/source/) |
| serde_yaml | 0.9.34+deprecated | YAML plans and registries | MIT OR Apache-2.0; [source](https://docs.rs/crate/serde_yaml/0.9.34+deprecated/source/) |
| uuid | 1.28.0 | Identifiers | MIT OR Apache-2.0; [source](https://docs.rs/crate/uuid/1.28.0/source/) |
| RustCrypto sha2 | 0.10.9 | Integrity hashes | MIT OR Apache-2.0; [source](https://docs.rs/crate/sha2/0.10.9/source/) |
| jsonschema | 0.58.6 | Tool argument validation | MIT; [source](https://docs.rs/crate/jsonschema/0.58.6/source/) |
| reqwest | 0.12.28 | MCP HTTP transport | MIT OR Apache-2.0; [source](https://docs.rs/crate/reqwest/0.12.28/source/) |
| url | 2.5.8 | URL handling | MIT OR Apache-2.0; [source](https://docs.rs/crate/url/2.5.8/source/) |
| Mozilla UniFFI | 0.28.3 | Rust runtime interface and Kotlin binding generation | MPL-2.0; [pinned upstream source](https://github.com/mozilla/uniffi-rs/tree/0d38fc51ec057c271233b70db51b13ef403e360d), [published source](https://docs.rs/crate/uniffi/0.28.3/source/) |
| MiniJinja and minijinja-contrib | 2.12.0 each | Model chat templates; optional `chat-template` / `llama` features | Apache-2.0; [MiniJinja source](https://docs.rs/crate/minijinja/2.12.0/source/), [contrib source](https://docs.rs/crate/minijinja-contrib/2.12.0/source/) |
| llama-cpp-2 and llama-cpp-sys-2 | 0.1.159 each | Optional `llama` inference backend | MIT OR Apache-2.0 for the Rust wrappers; [upstream](https://github.com/utilityai/llama-cpp-rs), [sys package](https://docs.rs/crate/llama-cpp-sys-2/0.1.159/source/) |

UniFFI is licensed under the [Mozilla Public License 2.0](https://www.mozilla.org/MPL/2.0/).
A binary distribution needs the applicable MPL notices and a way for recipients
to obtain its covered source, including any modifications. The upstream project
and exact version above identify that source; the release notice bundle must
record the actual source availability arrangement. Do not relabel UniFFI as
Apache-2.0 merely because trollm uses that license.

`Cargo.lock` also includes transitive libraries and host/test/build dependencies.
For example, the HTTP stack includes rustls 0.23.45 and ring 0.17.14; ring contains
third-party cryptographic code with its own notices. YAML parsing also brings
in unsafe-libyaml. These embedded components need their own source/license
inspection; the direct-dependency table does not cover them. Another thiserror
version, 1.0.69, is present transitively.

The following notice is reproduced from jsonschema 0.58.6's `LICENSE`:

```text
MIT License

Copyright (c) 2020-2026 Dmitry Dygalo

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Android application libraries

The declarations are in [the app build file](apps/android/app/build.gradle.kts).
Declared versions are not a substitute for a resolved runtime graph: Gradle can
select newer compatible transitive versions.

| Project / artifact | Declared version | Attribution and license source |
| --- | --- | --- |
| OpenID Foundation AppAuth (`net.openid:appauth`) | 0.11.1 | OAuth authorization; [Apache-2.0 license at 0.11.1](https://github.com/openid/AppAuth-Android/blob/0.11.1/LICENSE) |
| Java Native Access (`net.java.dev.jna:jna`, AAR) | 5.14.0 | Native bridge used by generated Kotlin bindings; [license at 5.14.0](https://github.com/java-native-access/jna/blob/5.14.0/LICENSE), [source](https://github.com/java-native-access/jna/tree/5.14.0) |
| AndroidX Core KTX | 1.16.0 | [AndroidX source and licensing](https://android.googlesource.com/platform/frameworks/support/) |
| AndroidX Activity Compose | 1.10.1 | AndroidX, Apache-2.0 |
| AndroidX Lifecycle Runtime Compose | 2.8.7 | AndroidX, Apache-2.0 |
| AndroidX Security Crypto | 1.1.0-alpha07 | AndroidX, Apache-2.0; inspect its resolved cryptography dependencies too |
| Compose UI, Material 3 and UI Tooling Preview | BOM 2025.04.01 | AndroidX, Apache-2.0; [official BOM mapping](https://developer.android.com/develop/ui/compose/bom/bom-mapping) |
| Compose UI Tooling | BOM 2025.04.01; debug only | AndroidX, Apache-2.0; include when distributing a debug APK |
| Kotlin runtime | Gradle Kotlin plugin 2.1.20; resolve actual runtime modules | [JetBrains Kotlin source and Apache-2.0 license](https://github.com/JetBrains/kotlin) |

The Compose BOM is a version constraint platform, not an additional UI runtime.
The [official April 2025 release announcement](https://android-developers.googleblog.com/2025/04/whats-new-in-jetpack-compose-april-25.html)
identifies Compose 1.8 with this BOM. Obtain individual UI/Material 3 artifact
versions from the selected runtime configuration rather than applying 1.8 to
every Compose artifact.

JNA's pinned license offers Apache-2.0 or LGPL 2.1 or later; the Apache-2.0 option
is available for this dependency. Its upstream license includes this notice:

> You can freely decide which license you want to apply to the project.

This choice does not replace notices for JNA's embedded native code, including
libffi. Inspect the actual AAR and its corresponding source release for those
notices. The Apache-2.0 license text is included locally in [LICENSE](LICENSE).

JUnit 4.13.2 and `org.json:json:20250517` are declared only for JVM tests.
Gradle, Android Gradle Plugin, CMake, the Rust compiler, bindgen and SDK/NDK build
tools are build inputs; their presence does not mean all their code is shipped.
Runtime libraries copied from those tools do need to be inventoried.

## Native code in the llama build

`llama-cpp-sys-2` compiles a vendored copy of **llama.cpp / GGML**. The wrapper
version, 0.1.159, is not a llama.cpp release number. Both locked Rust packages
record upstream wrapper commit `3cfdd729d65e35da407e5f820edf73201bfa54f6` in their
`.cargo_vcs_info.json`; see the [pinned wrapper source](https://github.com/utilityai/llama-cpp-rs/tree/3cfdd729d65e35da407e5f820edf73201bfa54f6).
The exact vendored source is included in the published `llama-cpp-sys-2` 0.1.159
crate archive, whose Cargo checksum is
`1a96d43d9370ff79293fe9777c9a4bda9d3bacb7100b61ed1e3690df39e9ad9e`.
This identifies the packaged source without substituting current `master`.
The pinned wrapper tree points to llama.cpp commit
[`26394b4e6749a41c3633db040e0987500a5f7013`](https://github.com/ggml-org/llama.cpp/tree/26394b4e6749a41c3633db040e0987500a5f7013).

The following text is reproduced from that exact package's `llama.cpp/LICENSE`:

```text
MIT License

Copyright (c) 2023-2026 The ggml authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

GGML and enabled bundled backend/third-party code must also be inspected for
additional notices when preparing the APK's complete notice bundle.

The Android build explicitly copies **`libc++_shared.so`** into each llama ABI
package from **NDK 27.2.12479018 (r27c)**. LLVM libc++ and its incorporated runtime
components require their own notices. Collect the installed NDK's license/NOTICE
material for the actual library and preserve its applicable LLVM and third-party
license texts. See the [NDK license guidance](https://developer.android.com/ndk/guides)
and [LLVM license](https://llvm.org/LICENSE.txt). An SBOM of Maven artifacts or
Rust crate names alone misses this shared library.

## Separately downloaded model weights

GGUF weights are separately fetched during model preparation; they are not Rust
or Android application libraries and are not covered by trollm's license. The
starter descriptors pin these files. No weights need to be downloaded to inspect
this attribution.

| Starter use | Original model author / upstream | Quantized artifact publisher and pinned source |
| --- | --- | --- |
| Text-summary, browser-summary and check-total coder | Qwen team, [Qwen/Qwen3.5-0.8B](https://huggingface.co/Qwen/Qwen3.5-0.8B) | Unsloth, [`Qwen3.5-0.8B-Q4_K_M.gguf` at `ed4bdcdbcce9f095bb49e8759176c64e45fa294b`](https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/tree/ed4bdcdbcce9f095bb49e8759176c64e45fa294b) |
| Browser-summary and check-total reasoner | Google DeepMind, [google/gemma-4-E2B-it](https://huggingface.co/google/gemma-4-E2B-it) | Unsloth, [`gemma-4-E2B-it-Q4_K_M.gguf` at `0314792d7f1f7e229411f620751375812bb9faf2`](https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/tree/0314792d7f1f7e229411f620751375812bb9faf2) |

The [pinned Qwen quantization model card](https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/blob/ed4bdcdbcce9f095bb49e8759176c64e45fa294b/README.md)
identifies Qwen/Qwen3.5-0.8B as the base model and Apache-2.0 as the license,
linking the [Qwen upstream license](https://huggingface.co/Qwen/Qwen3.5-0.8B/blob/main/LICENSE).
The [pinned Gemma quantization card](https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/blob/0314792d7f1f7e229411f620751375812bb9faf2/README.md)
identifies google/gemma-4-E2B-it as its base model and Apache-2.0 as its license,
linking [Google's Gemma 4 license](https://ai.google.dev/gemma/docs/gemma_4_license).

The setup descriptors record these SHA-256 values:

- Qwen: `bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517`
  (532,517,120 bytes).
- Gemma: `740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8`
  (3,106,738,272 bytes).

These pins identify the configured quantizations, not a verified derivation from
an exact original-model revision: the setup descriptors do not record that
revision or the quantization toolchain. Preserve the model's own license and any
notices if redistributing weights, and review those terms again when replacing a
setup pin or selecting a different model.

## Completing notices for an APK

The checkout provides [an Android SBOM exporter](scripts/android-sbom.gradle).
It resolves the selected `debugRuntimeClasspath` or `releaseRuntimeClasspath`
and emits a CycloneDX 1.5 graph of Maven component coordinates. It does not collect
licenses, copyright/NOTICE text, AAR-embedded native dependencies, Rust dependencies,
model weights or the NDK runtime. It also describes resolution, not an inspection
of the finished APK. No complete packaged-component notice inventory is included
with this source-level document.

Before distributing an APK, reconcile its actual contents with the Rust target
and feature graph and the selected Gradle runtime graph. Collect the exact
packages' license and notice texts, including cryptography, JNA/libffi,
llama.cpp/GGML and libc++; retain the pinned source identities and satisfy
covered-source availability requirements such as UniFFI's MPL-2.0. Bundle those
notices with the distributed artifact in an accessible form. Inspect separately
any redistributed GGUF files and retain their pinned provenance and notices.
This remaining work requires the actual build inputs and packaged artifact;
passing source tests does not complete it.
