# Product direction and current work

Reviewed on **2026-11-19**. trollm lets people run authored workflows with small
local models on Android. A flow defines the steps and tools. The user chooses a
flow, supplies its input, and follows the run to its report. Browser flows can
use E2LLM with a browser on another computer. [Learn trollm](LEARN-TROLLM.md)

## Available now

- **Android 1.1.0** (update code 72) is an experimental signed APK for Android
  13 or later. It brings inputs, model downloads and connection setup into the
  flow screen. Saved runs can be reopened, and completed runs lead with a
  selectable report and a copy button. [Download and verify the APK](INSTALL-ANDROID.md)
- **Bundled flows** include **Summarize text**, **Check a total** and **Browse and
  summarize**. The first two run locally once their models are installed. In
  **Check a total**, a model selects the amounts, the calculator adds them, and
  an authored direct tool task compares the saved sum with the budget.
  [Direct tool execution](DIRECT-TOOL-EXECUTION.md)
- **The runtime** follows authored steps, checks tool calls and outputs, and
  saves original evidence for later steps. A coder chooses explicitly granted
  tools where the flow calls for model judgment. A reasoner gives advice in
  ordinary text. The runtime uses authored transitions and verified choices.
  [Anatomy of a flow](ANATOMY-OF-A-FLOW.md) · [Flow reliability](FLOW-RELIABILITY.md)
- **The desktop flow review companion** is available as a public Linux/amd64
  container. It opens source files or bundles so people can inspect steps,
  prompts and tool permissions, leave feedback and compare revisions.
  Execution and inference happen in the Android app.
  [Deployment guide](../apps/flow-review/deploy/README.md) · [GitHub Packages](https://github.com/e2llm/trollm/pkgs/container/trollm-flow-review)

## Open GitHub issues

All seven issues below were open when reviewed. Follow [GitHub Issues](https://github.com/e2llm/trollm/issues)
for current status, labels and acceptance criteria. [Contributing](../CONTRIBUTING.md)
covers the issue and pull request process.

| Issue | What remains |
| --- | --- |
| [#1 Show file and field paths in example-validation errors](https://github.com/e2llm/trollm/issues/1) | Add actionable diagnostics and malformed-input tests for the current format, including invalid registry versions under `python3 -O`. Good first issue. |
| [#2 Validate final-report selection and copying on Android](https://github.com/e2llm/trollm/issues/2) | Check selection, partial and full copying, scrolling and reopening a completed report from history on a device or emulator with a stub build. The report already supports selection and has a dedicated full-copy action. |
| [#3 Validate run-control layout at narrow widths and large text](https://github.com/e2llm/trollm/issues/3) | The issue describes the earlier wrapping row. On 1.1.0, check Pause and Resume in the run screen and Cancel in the **Run actions** menu at the requested widths, font sizes and run states. A stub build is enough for these layout checks. |
| [#4 Add a deterministic packer for existing v1 flow bundles](https://github.com/e2llm/trollm/issues/4) | Package arbitrary current-format flow sources using the existing manifest and reader. The starter and replay packers already serve their specific inputs. |
| [#5 Add a small non-browser selection replay walkthrough](https://github.com/e2llm/trollm/issues/5) | The replay helper and tests are available. Add synthetic fixtures and a walkthrough for supported, unsupported and ambiguous selections using that helper. Good first issue. |
| [#6 Collect third-party license and NOTICE material for APK delivery](https://github.com/e2llm/trollm/issues/6) | The issue predates the generated notices and manifest in the 1.1.0 APK. Review what remains against that build and the [notice collector documentation](../THIRD_PARTY_NOTICES.md#notices-accompanying-an-apk) with the maintainer before duplicating the inventory. |
| [#7 Track temporary development-only picomatch exception — expires 2026-11-24 00:00 UTC](https://github.com/e2llm/trollm/issues/7) | Track remediation of the viewer's development dependency. The [approved exception](SECURITY-EXCEPTIONS.md) expires on **2026-11-24 at 00:00 UTC**. |

## What remains to validate

The [1.1.0 release](https://github.com/e2llm/trollm/releases/tag/v1.1.0) records
scripted workflow, JVM, Android lint, packaging and dependency checks. Physical
phone, TalkBack, real-model and live MCP acceptance for 1.1.0 remains open.
Current-build checks also need to cover OAuth refresh, ordinary model download
and recovery, and behavior in Doze. Issues #2 and #3 are bounded UI checks that
can be done with a stub build and no model downloads or live MCP connection.

The [two recorded v342 phone runs](../examples/run-342/README.md) document
particular runs on an earlier APK, with their exact flow and model files. The
bounded browser run and OAuth login recorded in the [Android guide](../apps/android/README.md)
used an earlier emulator build. Use the exact build and settings when reporting
new observations.

## Proposed authoring changes

Today's flows use the current [plan and bundle formats](../flows/README.md).
The [flow contract direction](FLOW-CONTRACT-DIRECTION.md) and the wider
[authoring pack](FLOW-AUTHORING.md) describe proposals. The packer in issue #4
works with the existing format and can proceed independently. Discuss changes
that affect readers or compatibility with the maintainer.
