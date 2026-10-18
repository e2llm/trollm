# Authoring a flow with an LLM

Status: **authoring direction, 2026-10-11**. The intended workflow below guides
actions F1–F4 in the [product plan](PRODUCT-PLAN.md). The new metadata and authoring
pack are not implemented yet. For executable material today, see the
[flow references and their limits](../flows/README.md).

Start with the [minimal summary reference](../flows/README.md#minimal-summary)
for a complete, directly authored v1 plan, registry and prompt. It requires no
browser recording or MCP connection. Its regression test uses a scripted model;
model acquisition, installable packaging and the full authoring pack remain ahead.

Running a supplied flow should require installation, account setup and any
starting input. Authoring a new flow requires a more detailed understanding of
the desired result and available tools. LLM assistance should make simple
processes routine to express from the specification.

## Start with the outcome

The requester supplies a plain-language goal and what a useful result looks
like. The authoring LLM proposes reasonable defaults, states assumptions and
asks only questions whose answers materially change the behavior.

For example:

> Recommend three interesting places near my current location. Prefer history
> and quiet outdoor spaces within a short walk. Explain why each is relevant.

Material questions might concern walking distance, whether to include paid
venues, or whether “open now” is required. Routine filenames and graph wiring
should come from the authoring pack. Explain unavailable capabilities through
their effect on the desired outcome and offer a workable alternative.

Success needs an observable definition: for example, return up to three places
from the retrieved candidate set with reasons tied to the supplied interests,
or explain that no suitable candidates were found. The model's recommendation
does not independently verify a venue's opening hours or current availability.

## Supply the authoring pack

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

## Iterate from requirements to a runnable bundle

1. **Clarify the result.** Capture required inputs, useful output, external
   actions, failure expectations and the few assumptions that matter.
2. **Map available capabilities.** Bind the required operations to supplied MCP
   tool contracts. Use local/remote providers according to the flow's needs.
3. **Author explicit steps.** Write prompts, tool/data bindings, completion and
   error behavior, limits and any iteration. Preserve structured facts in
   artifacts and use the model roles according to the execution contract.
4. **Package the procedure.** Include model/MCP defaults, title/input/result
   metadata and every referenced file. Let the packer compute real hashes.
5. **Exercise representative cases.** Use known inputs and fixture responses
   while developing, then the intended model/services. Keep fixture evidence
   distinguishable from real execution. These are authoring tests, not a new
   user-facing compatibility gate.
6. **Refine from diagnostics.** Give the LLM the failing step, relevant redacted
   configuration/error and artifacts. Fix the cause, rerun the affected case
   and record the accepted bundle version.

The runnable default is the author's deliverable. Users may override its
configuration without advance compatibility checks; runtime failures should
provide sufficient detail to diagnose what happened.

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

Moray, an optional browser-flow authoring project, can produce browser flows
under the same contract. Its recording-to-flow
process is an authoring option; direct authors can supply the procedure too.

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

Give an LLM only the published authoring pack, actual tool definitions and the
request above. It should produce a runnable bundle without access to trollm or
Moray source internals. A user should be able to install that bundle, supply its
inputs and see the declared result. Any essential behavior that still depends
on undocumented team knowledge is a gap in the pack.

The host simulation and eventual packing/check commands are secondary author
tools. Do not advertise proposed command names as available until implemented;
today's [flow guide](../flows/README.md) describes the current format and helpers.
