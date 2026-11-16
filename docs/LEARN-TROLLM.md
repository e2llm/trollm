# Learn trollm through one flow

trollm runs authored workflows on an Android device using small local language
models and explicitly granted tools. It gives a model a focused job, saves the
result, and carries the relevant evidence into the next job. Ordinary code owns
the procedure's transitions, exact operations, limits and recovery.

The purpose is to make useful work repeatable without asking a small model to
discover the whole method and remember every fact in one long conversation.
The flow carries the procedure; the models handle the judgments and tool choices
that remain. Local inference can avoid a cloud model call, but a remote tool or
browser still needs its service and network connection.

The current app is experimental. The [installation guide](INSTALL-ANDROID.md)
describes the downloadable 1.0.1 APK. This page walks through the source and does
not claim fresh device observations. The [project overview](../README.md#current-evidence-and-limits)
records the actual release and evidence limits.

## The request, procedure and run are different things

Consider the supplied **Check a total** starter. The user provides text describing
exactly two item amounts and a separate budget. For the example text “Two items
cost 39.99 and 40.10” and budget `80`, the arithmetic result should be `80.09`,
which is greater than the budget. No purchase or other external action is requested.

The input changes between runs. The procedure remains: select the two item
amounts, add them, compare the saved sum with the budget, assess the evidence and
report. A **flow** is that authored procedure and its configuration. A **run** is
one execution with particular inputs, model settings and saved state.

The result depends on several different kinds of correctness. The calculator
must add the supplied operands correctly. The model must choose the intended
operands. The runtime must pass the saved sum into the comparison without asking
a model to reproduce it. The final report must describe the evidence honestly.
One successful check does not establish all the others.

## Three roles cooperate

| Role | Work in this example | Boundary |
| --- | --- | --- |
| Coder | Select the two amounts, invoke granted tools, and later write the report | The name means the tool-capable model role; it does not mean that this task writes or executes arbitrary code |
| Reasoner | Read the original request and calculator results, then give advice | It has no tools; its prose is advice for a later model task, not an executable branch command |
| Runtime / executor | Run the graph, bind data, validate calls and outputs, record progress and enforce limits | It does not infer the intended amounts or interpret “looks good” as a successful business check |

The coder also performs judgment. The reasoner is useful for a separate advisory
question, not a compulsory supervisor for every call. A flow can use one or both
roles. The small [minimal-summary example](ANATOMY-OF-A-FLOW.md) uses a coder with
no tools and no reasoner.

The [Check a total plan](../flows/starters/check-total/check-total.flow.yaml)
contains this path:

```text
sum → compare → advise → report → done
```

These are authored node IDs. They do not come from a model deciding what procedure
to invent. Other flows can let a coder choose among declared exits with
`flow.choose`; the runtime validates and records the selected route. It never
selects a route by parsing a reasoner's prose.

## Follow the data through the actual starter

The [task registry](../flows/starters/check-total/check-total.tasks.yaml) and its
linked prompts specify what happens at each node:

| Node | Model work | Data supplied or preserved by the runtime |
| --- | --- | --- |
| `sum` | The coder selects two amounts from the input text and calls `local.calculate` | The operation is fixed to `add`; the returned calculation becomes `sum.outputs.calculation` |
| `compare` | The coder invokes the calculator with an empty model-owned argument object | The operation is fixed to `compare`; its left operand is the saved sum and its right operand is the separate budget input |
| `advise` | The tool-free reasoner assesses the selected amounts and explains the comparison | It receives the original input and both actual calculator payloads, not only another model's summary |
| `report` | A tool-free coder task writes the final explanation | It receives the original input, both calculations and the reasoner's advice |

For the example, the addition result is `80.09`; the comparison result is
`greater`. Those expected values illustrate the authored path. This text is not
a transcript of a successful model or phone run.

An **artifact** is saved working data, such as a tool response. A **binding**
passes a saved value into a later task or tool argument. Here the comparison's
binding prevents a model from recomputing or mistyping the sum. A prompt view can
show a bounded selection of structured fields without altering the retained data;
it does not itself filter candidates or establish their suitability.

If the first coder accidentally chooses the budget as one of the item amounts,
the calculator can return perfectly correct arithmetic for the wrong question.
The reasoner receives the original text so it can assess that selection, but
its advice is still model work requiring evaluation. This is why runtime output
checks and real-model quality tests answer different questions.

The starter has a limitation for missing or ambiguous amounts. Its `sum` task
uses `completion: tool-result`, which constrains native generation to one tool
call. The prompt's instruction to say that clarification is needed cannot produce
a valid completion under that contract. A missing call fails the task, while a
valid call with incorrectly selected amounts can still pass the arithmetic checks.
This starter illustrates the supplied-input path; it has no clarification or
decline route. To add a valid no-selection outcome, use the existing
[routing contract](ADAPTIVE-FLOWS.md), with an explicit `flow.choose` grant,
compatible completion mode and authored exit. The
[adaptive routing example](../flows/examples/adaptive-routing.flow.yaml) shows
such exits; prose alone does not create them.

## What the files contribute

| Source | Responsibility |
| --- | --- |
| Plan `.flow.yaml` | Nodes, transitions, task references, model slots, tool policies and run-wide limits |
| Registry `.tasks.yaml` | Reusable task definitions: model role, prompt, grants, argument bindings, generation bounds, completion mode and output contracts |
| Prompt Markdown | The immediate job, decisive evidence, uncertainty instructions and permitted call or route choices |
| Optional setup descriptor | Android input fields, pinned model files/settings and connection defaults; credentials are separate |
| Bundle manifest | The exact files and hashes admitted together for installation |

The Check a total [setup](../flows/starters/check-total/setup.json) collects text
and a decimal-string budget. It declares the model assets and no remote MCP
connection. `local.calculate` runs locally, although the coder still invokes it.
This does not introduce model-free tool nodes or a flow expression language.

Edit the registry-linked prompt to change a task's instructions. The plan's
`input.promptTemplate` is reserved and inactive in the current implementation.
A node's `promptSupplement` can add the current stage and evidence where its
contract supports it. The actual rendered request includes runtime material too,
so prompt testing must inspect that complete request.

These source files are not themselves an installable loose-YAML application.
Android admits a bundle with its manifest and dependencies. Use the maintained
builder/reader for the selected package; do not assume a proposed general
authoring pack or an invented command already exists. A flow using a newer
runtime capability also needs an APK that contains it.

## What stays on the device, and what can be elsewhere

The Kotlin/Compose app handles installation, input, model preparation, connection
credentials, foreground execution and history. Rust interprets the flow and
mediates model/tool work through the generated Android bridge. llama.cpp provides
the local model backend. Model weights and run data live in the app's storage.

Tools can be local or remote through MCP, the Model Context Protocol. A browser
flow can use E2LLM's extension/relay to inspect and act on a browser on another
computer. The browser service owns perception and readiness; trollm owns the
authored procedure and its local progress. The Check a total starter needs no
such browser connection when its model files are available.

Start/Resume is explicit. Opening the app does not load models or replay a run.
After Start/Resume, model preparation verifies pinned bytes and may overlap
preparation for up to two referenced slots; inference remains serial. See
[model preparation](MODEL-PREPARATION.md) for the actual limits and measurements.

Saved progress is also not a snapshot of the whole external world. Runtime
snapshots, artifacts and validated journals support the documented recovery path.
An uncertain write may already have happened, so resuming must not blindly repeat
it. A failed reporting connection is different from a failed browser operation.
Current recovery can stop conservatively where reconciliation is unavailable.

## Optional integrations

An authoring LLM or an external service can produce a package for trollm's native
contracts. The [Moray source connector](MORAY-SOURCE-CONNECTOR.md) is one optional
integration; trollm still owns local execution, limits and recovery. A producer
does not become a second controller for the same run, and publishing a package
does not establish its model accuracy or business outcome.

The [flow review companion](../apps/flow-review/README.md) lets humans inspect a
package and send feedback to its authoring LLM. It does not execute the flow.
The calculator example can be authored and run without these services or a
browser connection. Their integration details are separate reference material.

## Where to go next

You can now explain what a user supplies, what the three roles do, how evidence
moves between tasks, and what a successful run does and does not establish.
That is a complete stopping point for product orientation.

For source formats, [Anatomy of a flow](ANATOMY-OF-A-FLOW.md) deliberately starts
with a smaller, tool-free summary example so the plan/registry/prompt relationship
can be followed without Android or MCP setup. Then [Authoring a flow](FLOW-AUTHORING.md)
connects intent, task design and evaluation to a runnable package. For a first
hands-on change, follow its
[starter prompt exercise](FLOW-AUTHORING.md#make-one-small-change-to-an-existing-starter).
The [reading tracks](README.md) show complete routes, costs and stopping outcomes;
specialist contracts remain lookups for the feature you are changing.
