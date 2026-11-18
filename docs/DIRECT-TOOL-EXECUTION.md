# Direct execution of fully bound tool calls

Status: ordinary direct calls implemented in this source, 2026-11-17.
This capability is newer than the published 1.0.1 APK: use an APK built from
source containing it. Older readers reject the new field. Historical v342
bundles and logs remain unchanged.

## Why this is worth doing

The v342 registry has 408 task definitions. Of these, 357 have one granted tool,
`completion: tool-result`, one generation, and `modelFields: []`. All 357 use
the same prompt: “Please call the granted operation once with args:{}. The
runtime supplies its arguments and checks the result.”

Four of their plan nodes also have a prompt supplement. Excluding those nodes
conservatively, and excluding every routing/prose task, gives:

| Recorded run | Candidate calls / all model calls | Local tool calls | Remote reads | Ordinary actions | Recorded candidate model time |
| --- | --- | --- | --- | --- | --- |
| A04e | 499 / 578 | 265 | 226 | 8 | 148.17 minutes |
| S21 | 484 / 562 | 259 | 217 | 8 | 61.25 minutes |

These are sums of the existing `model_call.ms` records joined to the original
plan/node/task definitions. They identify inference work to investigate removing;
they are not measured speedups or predictions of new end-to-end durations.
Faster observations, retries, website state, cache freshness and readiness can
change the executed path. The two runs remain selected historical examples.

Sources: [v342 evidence](../examples/run-342/README.md),
[registry](../examples/run-342/flow/amazon-cart.tasks.yaml), and the
[fixed prompt](../examples/run-342/flow/prompts/fixed.md).

## Authoring boundary

Use an explicit task execution mode. Keep `task` as the graph node kind and keep
tool grants, bindings, output contracts and mutation policy in the registry.
Existing tasks retain their model execution semantics until their author opts in.
Do not infer author intent from an empty model argument object or prompt text.

For example, a registry task can declare:

```yaml
tasks:
  compare-total:
    execution: tool
    completion: tool-result
    tools: [local.calculate]
    toolArguments:
      local.calculate:
        modelFields: []
        fromVars:
          left: sum.outputs.calculation.result
        fromInput:
          right: budget
        fixed:
          operation: compare
    outputContract:
      outputs:
        comparison:
          fromArtifact: calculation
          format: json
          type: object
      requireArtifactComplete: [calculation]
```

The plan also needs a result-publication policy for `local.calculate`, naming
`calculation`; the complete example below includes it. An output contract
requires an artifact but does not create one.

An explicitly direct task has no model, prompt, generation limits or model loop.
It grants exactly one ordinary tool, closes model-owned arguments to an empty
set, and gets every argument from existing typed bindings or literal JSON.
Parameterless tools are allowed if their authoritative schema permits them.
Missing bindings, incomplete effective arguments and invalid schemas fail before
dispatch. Model outputs, authored routes, `flow.choose` and prompt supplements
are rejected for this mode. A later model task may assess the retained result.

`tool-or-route` remains a model decision even when its ordinary operation has
fully bound arguments: it can choose an authored exit instead of acting. A
single-tool `tool-result` task has no valid semantic-decline completion, but
silently changing old tasks would still change observable failure/retry behavior
and could discard authored prompt instructions.

## Execution contract

1. Both request origins share the existing executor’s checked dispatch operation.
   Both model-produced and authored calls use the same grant checks, typed
   bindings, gateway validation, preparation, mutation intent/result journal,
   acknowledgement, artifact accounting, output verification and transition path.
   A direct call must not become an alternate unguarded gateway path.
2. Each direct attempt consumes one execution step before resolving bindings or
   admitting result storage. Model tasks retain their existing charge after a
   returned generation. Thus direct error/retry cycles still obey `maxSteps`.
   Direct attempts use no model tokens or
   reasoner calls. Cancellation and parent/child deadlines keep their existing
   authority, including journaling a returned mutation before stopping.
3. A `direct_tool_step` event records a charged direct attempt, including one
   that fails before dispatch. It is not a `model_call` or proof of a tool effect.
   `run_metrics.py` adds `directToolSteps` only when these events are present;
   historical output remains unchanged. Actual tool results/failures retain their
   existing call association and counts.
4. Direct tasks have no prompt files or model preparation. A direct-only plan
   declares `models: {}` and can run without model files or an inference backend.
   Mixed flows prepare their actual model-task slots. Unused declared model slots
   retain legacy setup/configuration requirements; remove unused declarations to
   make a flow model-free.
5. Existing model-task behavior and saved-state interpretation are retained.
   New readers accept both execution modes; older APKs reject the new field.
   A new APK is required. Original v342 evidence is not rewritten.

## Selected-call boundary and ordinary recovery

This first implementation supports ordinary local/MCP calls, including journaled
mutations. Selected-guard tasks stay in their existing model mode. The production
gateway rejects direct calls requiring selected preparation before acquiring a
guard or consuming its context. Custom `ToolGateway` implementations must opt in
through the pure `validate_direct_call` capability check; its default refuses.
It does not replace argument/schema validation.

The selected path still incorporates actual `modelSlot` and `modelCallOrdinal`
into the native context and caller association. `CallOrigin::Model` preserves
its existing serialized fields and order. Ordinary direct calls have an explicit
`execution: tool` origin and no fabricated model identity. Supporting selected
direct calls later needs an approved caller-origin/custody compatibility contract.
No mode silently falls back to model execution or bypasses a selected guard.

Ordinary mutation failures retain the existing live recovery contract: an
unknown outcome is recorded, automatic `onError.retry` is rejected, and authored
re-observation or a pause may follow. Cold Resume refuses uncertain replay.
This is not the selected path’s permanent pending-guard suffix fence, and a
successful call or authored outcome is not proof of the business result.
Direct output contracts require artifacts returned by the current call and clear
old node aliases before an attempt; an old complete file cannot satisfy a new
producer that returns no artifact.

The present implementation sources are [executor](../crates/trollm-core/src/exec.rs),
[task registry](../crates/trollm-core/src/registry.rs),
[preparation context](../crates/trollm-mcp/src/preparation.rs), and
[selected caller association](../crates/trollm-uniffi/src/collection_provider.rs).

## Example, checks and migration

The [direct calculation example](../flows/examples/direct-calculation.flow.yaml)
and [registry](../flows/tasks/direct-calculation.tasks.yaml) add typed amounts
and compare the result to a budget with zero model slots and zero token budget.
Its input is `{"left":"39.99","right":"40.10","budget":"80"}`; it retains
`80.09` and `greater` in the original calculation artifacts/bound outputs.
It does not interpret prose or infer whether those were the intended amounts.

The [direct execution tests](../crates/trollm-core/tests/direct_tool_execution.rs)
exercise explicit direct and legacy model tasks side by side. They compare
effective calls and retained artifacts, including zero model
loads/generations for the direct-only example. Their cases cover
typed/missing bindings, wire-schema failures, stale result artifacts, cycles,
retry budgets, cancellation, read failures, mutation uncertainty and Resume.
The prepared-gateway controls cover rejection before guard acquisition; existing
shared dispatch/recovery tests cover deadlines and journal I/O failures.
Existing model and routing tests remain part of the regression suite.

A subsequent phone experiment should create a new bundle revision derived from
the v342 source, manually reviewing
the four supplemented nodes and each action. Preserve the original bundle and
logs. Use scripted equivalence checks before a separately identified phone/browser
trial. The browser service owns readiness: remove incidental model delays without
adding site-specific sleeps or parsing rules to the executor. Record model-call
counts, direct-call counts, outcome, failures and actual wall time in the trial.

The measured opportunity warrants a new phone trial; the historical timing
figures above do not establish the outcome or speed of that trial.
