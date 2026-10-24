# Working in trollm

## What we are building

trollm (pronounced “tro-le-lem”) is an Android app and Rust runtime for authored
workflows using small local language models. A user chooses a flow, supplies
input, and follows its progress. Tools run locally or through remote MCP
(Model Context Protocol) servers. A browser can be on another computer.

The design breaks a larger job into short model tasks. Ordinary code handles
exact computation, saved data, allowed transitions and execution limits. This is
an Android 13/API 33+ integration prototype. [README.md](README.md) records
current capabilities and acceptance gaps.

## The three roles

Here, `coder` and `reasoner` name model roles **inside the app**:

| Role | Job |
| --- | --- |
| **Coder** | Reads instructions and observations; calls explicitly granted tools or writes prose. |
| **Reasoner** | Receives evidence and a question; returns advice in ordinary text (prose). It has no tools. |
| **Runtime / executor** | Rust code that runs the graph, checks calls and outputs, saves results, and enforces step/token/time limits and recovery rules. Older docs also call it the “ledger.” |

A flow can use one or both model roles. The graph declares the possible next
steps. Some transitions are fixed; a coder can also choose an authored exit with
the `flow.choose` tool. The runtime validates and records that choice. It never
selects a branch by interpreting words in the reasoner's answer.

### Concrete example: Check a total

The existing [starter](flows/starters/check-total/) accepts text such as
“Two items cost 39.99 and 40.10” plus a separate budget of 80:

1. A coder selects the amounts and calls the phone's `local.calculate` tool to
   add them. The result contains `80.09`.
2. Another coder task calls the calculator to compare the saved sum with the
   supplied budget. Runtime argument bindings supply those exact values.
3. The tool-free reasoner receives the original text and both actual calculator
   results, then gives advice in prose.
4. A coder receives that evidence and advice and writes the final report.

The calculator performs arithmetic; the model selects what to calculate. Correct
arithmetic does not prove it selected the right amounts. Likewise, a successful
run means its declared checks passed, not that all model statements are true.

## What makes a flow

Start with [Anatomy of a flow](docs/ANATOMY-OF-A-FLOW.md), which walks through
the complete, tool-free minimal-summary example and its existing test.

| Source | What belongs there |
| --- | --- |
| `*.flow.yaml` (plan) | Graph nodes, transitions, run budgets and references to tasks. A node is one position in the graph. |
| `*.tasks.yaml` (registry) | Reusable task definitions: model role, prompt file, tool grants, generation bounds, completion mode and expected outputs. A node's `taskRef` selects a registry key. |
| Prompt `.md` files | Instructions for a model task, with bindings such as `{{ user_input }}`. A node's `promptSupplement` adds instructions for that step. |
| Optional `setup.json` | Android input fields, pinned model downloads/settings and connection defaults. Credentials stay outside it. |

An **artifact** is saved working data, such as an original tool result. A
**binding** passes a saved value to a later task without asking a model to
transcribe it. Preserve original evidence alongside model advice.

**Edit the registry-linked prompt to change model instructions.** The plan's
`input.promptTemplate` field is reserved and inactive.

Android installs a bundle containing the plan, registry, prompts and optional
setup, checked against a manifest of file checksums. Use the existing bundle generator and
reader; a loose YAML file is not an installable bundle. A new runtime capability
requires an APK containing its code, as well as a flow that uses it.

## Choose the right place to change

Workflow-specific steps and instructions belong in `flows/`. Reuse generic
tasks and tools. A missing general runtime capability needs a design discussion
with the maintainer before implementation.

| Change | Start here |
| --- | --- |
| A flow's procedure, prompts or model/setup defaults | `flows/`; [flow guide](flows/README.md) |
| Routing, validation, variables, artifacts or recovery | `crates/trollm-core/src/`; start with `plan.rs`, `registry.rs`, `validate.rs`, `exec.rs` |
| Tool dispatch, arguments, policies or an isolated built-in operation | `crates/trollm-mcp/src/`; `gateway.rs`, `policy.rs`, `local.rs` |
| Model calls, tool-call syntax, chat templates or llama.cpp | `crates/trollm-models/src/` |
| Android bundle/setup handling or the Kotlin-to-Rust API | `crates/trollm-uniffi/src/` |
| Screens, downloads, credentials or run lifecycle | `apps/android/app/src/main/` |

The Kotlin/Compose app calls Rust through a **UniFFI** generated bridge. Rust
uses llama.cpp for local inference. Browser starters use the separate E2LLM
service for browser perception and readiness; trollm coordinates its tool calls.

## Read for the task at hand

Read [CONTRIBUTING.md](CONTRIBUTING.md) for workflow/checks and
[PRODUCT-PLAN.md](docs/PRODUCT-PLAN.md) for priorities. Before changing behavior,
read [PREMISE.md](docs/PREMISE.md) and the relevant contract:

| Working on | Read next |
| --- | --- |
| Flow control, step instructions or generation limits | [ADAPTIVE-FLOWS.md](docs/ADAPTIVE-FLOWS.md); the executable `flows/examples/adaptive-routing.flow.yaml` and its registry |
| Built-in tools and calculator arguments/results | [BUILTIN-TOOLS.md](docs/BUILTIN-TOOLS.md) |
| Runtime behavior or formats | [ARCHITECTURE-ANDROID.md](docs/ARCHITECTURE-ANDROID.md), including its later implementation qualifications; relevant `schemas/`, reader and tests |
| Android | [Android guide](apps/android/README.md) |
| Security | [SECURITY.md](SECURITY.md) and [enterprise security](docs/SECURITY-ENTERPRISE.md) |
| Human flow review / self-hosted viewer | [Flow review design and implementation plan](docs/flow-review/README.md); isolated application in `apps/flow-review/` |

If `docs/AMAZON-DEVELOPMENT.md` exists, read it for that branch's Amazon flow,
host checks and phone-owner handoff. Keep its business logic in the flow.

`docs/ADAPTIVE-FLOWS.md` describes **implemented** routing.
`docs/FLOW-CONTRACT-DIRECTION.md` and `docs/FLOW-AUTHORING.md` contain
**proposals**; their proposed fields may not exist.
Check current schemas, readers and architecture qualifications; report conflicts
before changing a consequential contract. Minimal-summary and ticket-triage have
scripted tests; procurement is an incomplete design sketch.

## Design rules to preserve

- Keep computation in isolated coder-invoked tools and procedure in the graph.
  Do not add site/task-name special cases to the executor, model-free tool nodes,
  or a flow expression language.
- Model prose stays advisory. Structured model output goes through the validated
  tool-call boundary; do not require a model to emit JSON worklists or magic
  completion words. The reasoner remains tool-free.
- `local.calculate` runs on the phone; `flow.choose` is handled by the runtime.
  Both need explicit task grants. Neither is a remote server or setup connection.
- Validate output/artifact contracts before transitioning. Keep bundles and
  model files pinned by SHA-256 checksums. Preserve original results through bindings.
- A failed or interrupted write may already have taken effect. Preserve mutation
  journaling and conservative recovery; observe before attempting another write.
- Models load on first use. Opening the app does not preload models or replay
  runs. Lost workers require explicit Resume; Android owns foreground execution,
  credentials and persistent app data.

## Make and check a change

Check `git status --short` and the current branch; preserve others' edits. Find
the source and nearest test using the map above. The main agent/maintainer owns
specification, architecture and product decisions. Sub-agents raise material
departures before implementation. Finish routine approved work without
re-requesting granted permissions.

Keep Rust and Android together. Edit sources; generate bindings and starter
archives with the documented tools. Keep builds, weights and local environments
out of Git. Update schema, reader, examples and consumers together for approved
format changes. Follow [versioning](docs/VERSIONING.md) for release identities.

**Check RAM and disk before compiling or downloading.** Reuse installed tools
and bound concurrency. On constrained hosts, leave Android/llama.cpp builds to
the existing build environment; do not download weights or SDK/NDK installations.
Device work needs the configured ADB connection, an explicit serial and one owner.

Use checks for the changed layer. `CONTRIBUTING.md` has full commands;
`rust-toolchain.toml` pins Rust and `scripts/requirements.txt` lists Python dependencies.

- Docs only: check accuracy, links and the diff; use export validation when
  present. No Rust or Android build is needed.
- Flow changes: run `python3 scripts/validate-examples.py` and the matching core
  integration test. For the smallest example:
  `CARGO_BUILD_JOBS=2 cargo test --locked -p trollm-core --test direct_authoring --no-default-features`.
- Packaged starters: regenerate with `scripts/android/make_starter_bundles.py`,
  check with its `--check` flag, and run the relevant bundle/helper tests.
- Rust changes: run relevant tests, formatting and Clippy from `CONTRIBUTING.md`.
  Bridge host tests use `--features stub`; a bridge with neither `stub` nor
  `llama` compiles but cannot start model runs.

Scripted/stub tests need no weights and test wiring and execution rules. Report
whether checks used scripted models, JVM tests, real models or live MCP; one does
not establish the others. Run relevant checks once changes settle. Avoid long
CI polling loops.

## Public and private checkouts

This file applies to both. Public source has curated history and fewer operator
files. Use local `CONTRIBUTING.md` for the contribution destination and any export
manifest/overlays present here. Portable checks need no omitted private tooling;
source publication does not publish APKs.

Remote tools can transmit data and require connectivity. Keep credentials,
private endpoints, device details, caches and unsanitized diagnostics out of Git
and public reports. Use placeholders and existing credential storage. Follow
`SECURITY.md` for private reporting and existing maintainer authorization for
merges, deployment and publication.
