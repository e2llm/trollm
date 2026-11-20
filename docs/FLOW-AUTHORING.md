# Authoring a flow with an LLM

Start from the outcome, design focused tasks, then express them in the current
plan/registry/prompt contracts. This guide explains that authoring method and its
model-testing loop. It distinguishes today's individual contracts and helpers
from the proposed complete authoring pack.

[Learn trollm](LEARN-TROLLM.md) gives the product picture through Check a total.
[Anatomy of a flow](ANATOMY-OF-A-FLOW.md) supplies the smaller current-format source
walkthrough. Follow the [small-model practices](FLOW-PRACTICES.md) and
[execution-model prompting method](FLOW-PROMPTING.md) for concrete task patterns
and behavioral comparisons; they are usable independently of the future pack.

## Start with the outcome

The requester supplies a plain-language goal and what a useful result looks
like. The authoring LLM proposes reasonable defaults, states assumptions and
asks only questions whose answers materially change the behavior.

For example:

> Recommend three interesting places near my current location. Prefer history
> and quiet outdoor spaces within a short walk. Explain why each is relevant.

Material questions might concern walking distance, whether to include paid
venues, or whether “open now” is required. Reuse current examples and contracts
for routine filenames and graph wiring; the proposed complete authoring pack is
not a prerequisite. Explain unavailable capabilities through their effect on the
desired outcome and offer a workable alternative.

Success needs an observable definition: for example, return up to three places
from the retrieved candidate set with reasons tied to the supplied interests,
or explain that no suitable candidates were found. The model's recommendation
does not independently verify a venue's opening hours or current availability.

Intent does not follow from a scene or an available tool. An apple next to a
basket is not an instruction to move it. A found defect is not a request to
repair it. Write down material assumptions and clarify those that could change
the requested outcome; ordinary implementation choices can use reviewed defaults.

## Design closed tasks before tuning prompts

A useful task has one immediate job, enough evidence to do it and an explicit
meaning for its result. Use this contract while authoring; it is a checklist,
not a set of new execution-plan fields.

| Concern | What to establish |
| --- | --- |
| Stable knowledge | Terms, units, product rules and tool meanings needed by this task family/version |
| Run inputs and observations | Original request and parameters, current source facts, subject identity and freshness |
| Immediate intent | The question this step answers and how it contributes to the flow |
| Method boundary | Information sources and choices it should use, including the point at which it should stop or decline |
| Actual tools and bindings | Granted operations, runtime-bound arguments and the values the coder must select |
| Result and evaluation | Expected artifact or prose, structural checks, semantic criteria and downstream consumers |
| Limits and alternatives | Generation/run bounds, missing-data behavior, authored routes, failure and recovery |

For Check a total, the first task selects two item amounts; the second compares
an already saved sum with a supplied budget. They should not independently
reinterpret the whole mission. The reasoner reviewing the choice needs original
facts; a task merely forwarding advice has a different job. Preserve these
contracts in normal and recovery paths.

A closed task may inspect several records or make supported calls, but it is not
expected to invent a different mission to force an answer. “Check the supplied
records” does not authorize a search elsewhere when the records are insufficient.
An available tool is not automatically part of the expected method. An explicit
no-selection route and a tool/transport failure are different outcomes; use the
current completion and routing contracts to represent them.

Information acquisition, analysis, decision and execution are useful stages for
finding task boundaries. They need not be four model calls or new node types.
Keep a coherent job together when the selected model can perform it reliably;
split where evidence, capability or recovery requires a boundary. The current
`loop.maxSteps` field bounds model generations. It is not a semantic measure of
how much problem solving a task contains.

Test the task with its intended inputs and model settings. Making an assignment
short does not by itself establish reliability, and an exact tool can calculate
correctly even when the model chooses the wrong operands.

Trollm's local-model design favors closed units and authored control flow.
Do not assume autonomous subagents, unrestricted replanning or a frontier helper
are available. A broader process can require external judgment, but it needs an
explicit supported interface and retains that dependency; hiding it behind a
tool name does not make its solving work algorithmic.

Choose the design for the goal's useful outcomes, coverage and resource needs.
Correct answers for a tiny known set plus unknown everywhere else can be cheap
and predictable without being useful enough. Count appropriate and unnecessary
declines separately from correct and wrong selections. Extra reasoning or an
external step can be worthwhile when supported and when its measured benefit
justifies the cost; it does not permit changing hard requirements or tool authority.

## Iterate from requirements to a runnable bundle

1. **Clarify the result.** Capture required inputs, useful output, external
   actions, failure expectations and the few assumptions that matter.
2. **Map available capabilities.** Bind required operations to actual built-in
   or MCP tool contracts. Use local/remote providers according to the flow's needs.
3. **Author explicit steps.** Write prompts, tool/data bindings, completion and
   error behavior, limits and any iteration. Preserve structured facts in
   artifacts and use the model roles according to the execution contract.
   Use the intended coder and reasoner separately to
   [help phrase their own tasks](FLOW-PROMPTING.md). Review suggested wording
   against the contract before testing it; an explanation is not verified data.
4. **Package the procedure.** Include model/MCP defaults, title/input/result
   metadata and every referenced file. Use the maintained builder for a named
   starter and let it compute real hashes; the exercise below shows the commands.
   Arbitrary flows still need a producer implementing the existing bundle format;
   there is no general packing CLI yet.
5. **Exercise representative cases.** Actively test ambiguous interpretations
   that could change the result: product/accessory, names, currencies, units and
   addresses. Use known inputs and fixture responses while developing, then the
   intended model/services. Keep fixture evidence
   distinguishable from real execution. These are authoring tests, not a new
   user-facing compatibility gate.
   Compare actual selections and declines against a fixed control, including
   supported, unsupported and unseen cases. Freeze correct advice when isolating
   a coder handoff; check adviser accuracy separately before assessing the full flow.
6. **Refine from diagnostics.** Give the LLM the failing step, relevant redacted
   configuration/error and artifacts. Fix the cause, rerun the affected case
   and record the accepted bundle version.

The runnable default is the author's deliverable. Users may override its
configuration without advance compatibility checks; runtime failures should
provide sufficient detail to diagnose what happened.

## Make one small change to an existing starter

Try a report-wording change to **Check a total**. The host checks need Python,
the [documented dependencies](../CONTRIBUTING.md#validate-a-change-locally) and
the pinned Rust toolchain. Check RAM and disk first. They need no Android SDK,
model weights or MCP service.

1. Open the [task registry](../flows/starters/check-total/check-total.tasks.yaml)
   and find `report-cart`. It points to
   [prompts/report.md](../flows/starters/check-total/prompts/report.md), a tool-free
   coder task that receives both calculations and the reasoner's advice.
2. Keep its existing evidence and uncertainty instructions, and add a sentence
   such as: “Put the computed total and budget comparison in the first sentence;
   explain any uncertainty after it.” This edits the model's instructions through
   the registry-linked file. Keep the graph, tool grants and bindings unchanged
   for this exercise.
3. From the repository root, rebuild the named starter and run its checks:

   ```sh
   python3 scripts/android/make_starter_bundles.py --starter check-total
   python3 scripts/android/make_starter_bundles.py --check
   python3 scripts/validate-examples.py
   CARGO_BUILD_JOBS=2 cargo test --locked -p trollm-core --test local_starter --no-default-features
   git diff -- flows/starters/check-total apps/android/app/src/main/assets/flows/check-total.tar
   ```

The builder reads the exact source bytes and writes
`apps/android/app/src/main/assets/flows/check-total.tar`, including the updated
prompt and manifest hashes. Review both the prompt change and the regenerated
archive; do not hand-edit hashes. This builder supports the named starters, not
an arbitrary new flow directory.

The schema and archive checks establish valid packaging. The Rust test runs the
real executor and local tools with scripted model responses, checking saved-value
bindings and evidence delivery. It does not test whether a real coder follows
your new wording. That needs a compatible app with the intended models and the
rebuilt bundle, following the [Android guide](../apps/android/README.md). Compare
the actual report with the saved calculations and advice; a readable report is
not independent proof that the model selected the right input amounts.

## Human review of LLM-authored packages

The [flow review companion](flow-review/README.md) is the human review
interface. Its [implementation plan](flow-review/IMPLEMENTATION.md) records
delivery status. Source authoring remains with the LLM: humans inspect the
procedure, comment on relevant phases or steps, and return exported Markdown
feedback. The LLM updates the executable files and explanations together.

For each package, also supply `flow-review.json` using the
[presentation contract](flow-review/INTERFACE.md). Describe the business purpose,
required inputs, expected result, and a small number of meaningful phases.
Associate each phase with actual `(flow ID, node ID)` pairs, including recovery
and terminal nodes. Use the [Check a total example](../flows/starters/check-total/flow-review.json)
as a template. Keep this optional presentation file outside execution-plan YAML;
unknown presentation fields must not be inserted into the runtime schema.

Explain decisions and consequences in terms a requester can review. Distinguish
intent from guarantees: tool grants, routes and budgets come from the executable
source, while descriptive text explains why those steps were authored. Do not
describe an action or check that the executable package does not contain. Keep
model advice distinct from tool evidence and independently verified results.

When responding to feedback, locate the package revision and referenced node,
task, prompt or field. Preserve stable IDs where their meaning remains the same.
Update the package, phase references and descriptions; run the applicable
validators/tests; return the revised package and a short account of changes.
Feedback on an older revision requires checking the current source before
applying it. Presentation metadata does not change execution semantics.

The viewer accepts the presentation alongside a source directory. The current
named Android starter builder does not automatically package extra source files;
when another bundle producer includes the presentation in a bundle, list and
hash it in that bundle's existing manifest like every other member.

## Existing runtime and proposed authoring work

For the implemented coder-directed routing and node-context extension, use
[Coder-directed flows](ADAPTIVE-FLOWS.md). Its fields, schemas and readers govern
current execution. The current prompt-development and testing method can be
used today with the existing examples and tools.

The broader authoring-pack direction dates from 2026-10-11 and guides F1–F4 in
the [product plan](PRODUCT-PLAN.md). A complete integrated authoring pack and
proposed catalogue/format additions are separate work; a field appearing in
[Flow contract direction](FLOW-CONTRACT-DIRECTION.md) is not thereby implemented.
The optional flow-review presentation has its own existing contract and does
not add fields to the runtime plan.

For executable material today, use the [flow references and their limits](../flows/README.md).
[Anatomy of a flow](ANATOMY-OF-A-FLOW.md) follows the actual minimal-summary
plan, registry, prompt and scripted test. It needs no recording or MCP connection.
Actual model execution and Android installation still need their supported
model/configuration and bundle paths. Named starter and replay helpers are not
a general packer for every possible v1 flow.

Running a supplied flow should require its installed assets, applicable account
setup and starting input. Authoring a new flow needs a more detailed account of
the desired result and available tools. Do not make a future convenience pack a
reason to stop improving the current-format method.

## Supply the authoring pack

The table describes what a complete pack should assemble. Today these inputs
are distributed across the maintained contracts, examples and tool definitions;
this section does not claim a ready automated pack or a new runtime interface.

| Input | Why the LLM needs it |
| --- | --- |
| Versioned ICD and schema | Allowed fields, primitives, bindings and error/completion semantics |
| Complete small examples | Working bundles including all registries, prompts and subflows |
| Actual MCP tool definitions | Real operation names, argument/result shapes, account requirements and expected errors |
| Model presets and prompt conventions | Logical roles, exact acquisition/configuration defaults and the supported output conventions |
| Example inputs, tool responses and expected results | Concrete behavior and boundary cases rather than syntax alone |
| Runtime diagnostic examples | How to identify and fix a failed node, binding, model load or tool operation |
| Packing and local execution instructions | A reproducible route from authored files to the same bundle the app installs |

Tool definitions are authoring material, not a compatibility probe at each run.
Do not invent a tool or promise access to a service simply because the requested
workflow would benefit from it. Identify a missing provider explicitly.

## Worked design: nearby recommendations

This table is a design example, not executable YAML or a declaration that the
listed providers already exist in trollm.

| Step | Input | Work and output |
| --- | --- | --- |
| Starting input | Interests, distance preference, optional location override | App collects values using the flow's form/defaults |
| Location | User-selected location or device-location tool | Store coordinates and relevant source/freshness information as an artifact |
| Enrichment | Coordinates and search constraints | Call the chosen places MCP service; store candidate records and returned facts |
| Reasoning | Candidate data and the user's meaning of “interesting” | Apply the model/prompt arrangement declared by the flow; produce recommendations and explanations |
| Result | Recommendation prose and relevant candidate artifacts | Show a readable result in the app, including an explicit empty result when appropriate |

A local Android MCP adapter is one possible way to expose device location.
The example needs an actual implementation and its permission/error behavior
before it can claim live GPS support. Manually supplied coordinates or fixtures
are useful authoring inputs, with their scope clearly labelled.

Also exercise unavailable location, no candidates, an authentication failure,
a failed model call and cancellation. Preserve evidence for factual statements;
do not introduce a requirement that advisory model prose becomes deterministic
control flow or must obey a newly invented JSON protocol.

## Browser flows and recorded work

Moray is an optional procedure-authoring and execution-transfer system. It can
use recorded work and declared intent to propose reusable procedures and assess
model/runtime choices. Its adapter must still produce a supported Trollm package.
Direct authors and other producers use the same native contracts; unrelated
Trollm flows need no Moray account, shared database or per-action controller.

A recording shows what happened in that execution. Repeated actions on two
items do not establish whether the intention was “all items”, “these two” or
“the first two”. The author must specify collection membership, termination,
success evidence and recovery for a reusable procedure. Preserve the source
evidence and assumptions when generalizing recorded work.

The requester/implementer method above starts from the business outcome, asks
meaningful questions, states defaults, discovers capabilities and presents
understandable alternatives. This authoring contract serves the tools declared
by each flow.

## Acceptance for the authoring experience

This is the acceptance target for the proposed complete authoring pack, not a
claim that the integrated experience already exists.

Give an LLM only the published authoring pack, actual tool definitions and the
request above. It should produce a runnable bundle without access to trollm or
Moray source internals. A user should be able to install that bundle, supply its
inputs and see the declared result. Any essential behavior that still depends
on undocumented team knowledge is a gap in the pack.

The host simulation and eventual packing/check commands are secondary author
tools. Do not advertise proposed command names as available until implemented;
today's [flow guide](../flows/README.md) describes the current format and helpers.
