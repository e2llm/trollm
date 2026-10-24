# Built-in tools: interface contract v1

Calculator design agreed 2026-10-21; literal extraction added 2026-10-24.
This contract accompanies `local.calculate` and `local.extract_decimal`.
It defines source capabilities; older APKs do not gain them by
installing a new flow bundle. Build an APK containing the implementation before
claiming phone acceptance. The fixture and scripted checks below require no
Android device, model weights or live MCP server.

## Execution and ownership

A coder task requests a deterministic tool through the existing tool-call fence.
The ordinary executor checks its task grant and argument bindings; the gateway
validates and executes the operation, applies result policies and publishes
artifacts. No new node kind, implicit call or task-name dispatch is introduced.
The reasoner remains tool-free. Its prose is advice for a subsequent coder task;
the ledger never parses that prose into a decision or a tool target.

`local` is a reserved built-in server name. It needs no URL, connection, token,
download or network listener. Do not declare it in `mcp.servers` or setup
`connections`; a remote server named `local` is rejected. Declare the exact tool
in the task grant (`tools: [local.calculate]`). Tools from ordinary MCP servers
continue to use the existing connection and policy configuration.

The same operation is usable behind a remote MCP tool when needed, but a remote
tool has its own declared server identity. The `local` name always means the
on-device implementation, including in tests.

## Calculator contract

The version is the tool's documented argument/result contract and the result's
`format: trollm.local.calculate.v1`. This does not change the execution-plan or
task-registry v1 formats. The machine-readable contracts are
[input](../schemas/local-calculate.v1.input.schema.json),
[output](../schemas/local-calculate.v1.output.schema.json) and
[shared cases](../schemas/local-calculate.v1.cases.json).

Request exactly three fields:

| Field | Meaning |
| --- | --- |
| `operation` | `add`, `subtract`, `multiply`, `divide` or `compare` |
| `left` | Signed decimal string; first operand |
| `right` | Signed decimal string; second operand |

Decimals contain at most 12 integer digits and 6 fractional digits. Use an
optional minus sign, an integer part, and an optional dot/fraction. No leading
zeros except the integer `0`; no whitespace, plus sign, separators, units,
exponents, JSON numbers or expressions. Negative zero is accepted as input.
Operations are exact; this tool does not round or convert currencies/units.

Results have exactly `format`, `operation`, `left`, `right` and `result`.
`operation`, `left` and `right` echo the supplied values, preserving the decimal
strings for the next model's inspection. Arithmetic `result` is a canonical
decimal string: no redundant fractional zeros or decimal point, and zero is
`"0"`. Its magnitude cannot exceed `999999999999.999999`. A result needing more
than six fractional digits is rejected. Comparison returns `"less"`, `"equal"`
or `"greater"` for the left operand relative to the right.

```text
<tool_call>{"server":"local","tool":"calculate","args":{"operation":"add","left":"39.99","right":"40.10"}}</tool_call>
```

```json
{"format":"trollm.local.calculate.v1","operation":"add","left":"39.99","right":"40.10","result":"80.09"}
```

A following comparison with `left: "80.09"`, `right: "80"` returns `greater`.
The flow decides whether equality is permitted; `compare` does not interpret
phrases such as "under budget" or implement a shop-specific spending policy.

Invalid argument objects are rejected before dispatch. The calculator's own
validation uses `local.calculate: invalid-arguments`. Gateway schema validation
may instead report its existing bounded field-path diagnostic. Arithmetic
failures have stable details `local.calculate: division-by-zero`,
`local.calculate: precision-exceeded` or `local.calculate: out-of-range` and
produce no successful tool result. Existing executor error classes remain:
argument validation uses `tool-call-parse`; execution errors use `mcp-error`.
Do not turn an error, truncated model generation or missing result into success.

## Literal decimal extraction contract

`local.extract_decimal` is an optional coder-invoked read tool. It has no site
rules, browser access or semantic price validation. Its machine-readable contracts
are [input](../schemas/local-extract-decimal.v1.input.schema.json) and
[output](../schemas/local-extract-decimal.v1.output.schema.json). Grant it explicitly
with `tools: [local.extract_decimal]`; no setup connection is needed.

Supply exactly `primary`, `secondary` and `rules`. Both sources are required
strings, each at most 16,384 UTF-8 bytes. Use an empty string for unavailable
source text. Rules accept only these authored fields:

| Field | Meaning |
| --- | --- |
| `prefix` | Required nonempty literal immediately before the numeric text, allowing intervening whitespace. |
| `fractionDigits` | Required integer from 1 through 6; exactly this many fractional digits must match. |
| `after` | Optional literal; keep the text after its first occurrence. A missing marker leaves this source unmatched. |
| `before` | Optional literal; keep the text before its first occurrence within the remaining window. A missing marker leaves this source unmatched. |
| `stopBefore` | Optional array of at most 16 nonempty literals; cut the window at the earliest occurrence, using ASCII case-insensitive matching. |
| `minimumExclusive` | Optional signed calculator-format decimal string; every numeric match must be strictly greater. |

Markers and prefix are limited to 256 UTF-8 bytes each. `after`, `before` and
`prefix` are case-sensitive. JSON Schema `maxLength` counts characters; additional
standalone and gateway admission validation enforces the stricter UTF-8 byte
bounds before dispatch. The sources' text is never interpreted as JSON, HTML,
URLs or selectors.

Within each window the tool examines literal prefix occurrences and accepts an
unsigned integer of at most 12 digits, without redundant leading zeros, followed
by either a decimal point or whitespace, then the exact fractional width. For
example, with prefix `$` and width 2, both `$2.90` and `$2 90` yield `"2.9"`.
There is no locale conversion, exponent support, rounding or inference about
currency. Malformed candidates do not match. An additional dot immediately after
the fraction prevents that candidate from matching. Other trailing text is not
interpreted. Authored rules must scope the input appropriately.

Two matches in one source are ambiguous even when their numeric values are equal.
If each source has one match, their exact numeric values must agree. **One matching
source is sufficient for success**, with the other explicitly marked `unmatched`;
this is not two-source corroboration. `minimumExclusive` checks only actual
matches: it cannot turn a missing or malformed source into corroboration or prove
that a matched value is a current product price. A matched value at or below the
bound fails even if another candidate or source would pass. The tool does not
filter low values to select a preferred candidate.

The result contains `format: trollm.local.extract_decimal.v1`, the canonical
`decimal` string and two `sources` records in primary/secondary order. Each record
contains `role`, `textSha256` of the entire original source, `status` (`matched` or
`unmatched`) and `decimal` (canonical string or null). Hashes identify inputs; they
do not save the source text. Preserve original observations separately and bind
the extraction artifact at the task boundary when later tasks need it.

Stable standalone errors are `local.extract_decimal: invalid-arguments`,
`local.extract_decimal: no-match`, `local.extract_decimal: ambiguous-source`,
`local.extract_decimal: sources-disagree` and
`local.extract_decimal: outside-authored-bound`. Gateway schema validation may
instead report the existing bounded field-path diagnostic for invalid arguments.
No failure produces a successful tool result. Existing executor error classes,
read policies, setup constraints, task grants, argument bindings, audit redaction
and original-result artifact publication apply as for the calculator.

Focused tests use literal fixtures and scripted model calls, without weights or
network:

```sh
cargo test --locked -p trollm-mcp --no-default-features --test extract_decimal
cargo test --locked -p trollm-core --no-default-features --test local_decimal
```

## Policies, bindings and artifacts

Both built-in tools are read-class operations. Their policies support the existing
`resultArtifact`, `resultValidation` and `shape` behavior, with ordinary limits
and the single-attempt retry defaults. Mutation classification, passthrough,
cache, pagination and `serveFrom`/`onMiss` policies are rejected for local tools.
There is no reason to cache this bounded arithmetic or disguise it as a mutation.

Normal task grants, setup argument constraints and task `toolArguments` apply.
Task bindings still require `completion: tool-result`; this work does not relax
that contract for model-prose loops. Use `fixed` for authored operations/budgets
and `fromVars` for previous tool results, or `fromInput` for a top-level field in
JSON user input. The coder can omit bound arguments,
but conflicting values fail before dispatch. Unknown local tools fail closed.

For example, the sum task can publish `calculation.json`; a following comparison
binds `left` from the sum task's stored result and fixes `right` to the limit.
The sum's bound output is preserved even if another call replaces the named
artifact. A named artifact always contains the latest successful publication;
it is not an append-only calculation transcript. Bind each result at its task
boundary when later steps need both. An invalid replacement is never published
as a new complete artifact.

The existing gateway result pipeline preserves the unshaped result, validates
any declared result schema and writes its completeness sidecar. Main events
record the tool result and artifact; proxy audit records `local_call` without
raw arguments, and tool provenance is `Local`. Remote calls retain their current
authentication, caching, validation and mutation behavior.

## Prose, calculation, reasoning and action

1. Obtain cart prose from a browser tool and preserve its original returned text
   in an artifact. A coder commentary does not replace that source.
2. Give the coder that prose and a bounded calculation task. It selects operands
   through the tool-call boundary; no general page-to-record extraction is needed.
3. Give the reasoner the original prose and the actual calculator operands/results.
   It judges whether the selected facts and the arithmetic address the request,
   describes uncertainty, and returns ordinary prose.
4. Give the coder that advice and the original evidence. It may perform the next
   granted operation or seek clarification according to the authored flow.

Arithmetic is exact for the supplied operands. Operand selection and semantic
judgment remain model work and are not proven by a successful calculator call.
The calculator does not establish that a number is the current product price,
that currencies match, or that an item is suitable. Missing/ambiguous evidence
needs a bounded clarification or unresolved outcome. Browser mutations still
require current targets, intent logging and explicit handling of uncertain writes.

The [prose-calculation example](../flows/examples/prose-calculation.flow.yaml)
uses supplied text instead of a live browser and a fixed teaching budget of 80.
Its scripted executor test exercises the real local tool, artifact bindings and
both model roles; it does not establish real-model operand selection or phone
performance. Run it with:

```sh
cargo test --locked -p trollm-mcp --no-default-features
cargo test --locked -p trollm-core --test prose_calculation --no-default-features
python3 scripts/validate-examples.py
```

## Installable calculator starter

The [Check a total starter](../flows/starters/check-total/check-total.flow.yaml)
uses the same four tasks with a user-supplied budget. Android installs it with
the other bundled starters. Enter text describing exactly two item amounts and
a separate decimal budget; for example, `Two items cost 39.99 and 40.10.` with
budget `75`. Its [setup](../flows/starters/check-total/setup.json) reuses the
browser starter's checksum-pinned coder and reasoner models. No MCP connection,
token or browser is required; prepare both models before the first run.

The sum task fixes `operation: add` and lets the coder select the two operands.
The comparison task binds `operation: compare`, the saved sum via `fromVars`,
and `right` via `fromInput: budget`. The model requests the comparison with
`args: {}`. Both exact payloads and the original input reach the tool-free
reasoner and final coder; the coder also receives the reasoner's advice.
No calculation or model prose authorizes an external action.

The prompts explicitly teach the `<tool_call>` envelope. The runtime constrains
and validates calls but does not insert a tool catalog or tool-use instructions
into an authored prompt. Include the envelope and argument meaning when writing
other local-tool flows. Missing or invalid operands fail the task rather than
yielding a verified calculation. There is no interactive clarification step in
this starter; correct the input and start a new run if it cannot select two amounts.

```sh
python3 scripts/android/make_starter_bundles.py --check
cargo test --locked -p trollm-core --test local_starter --no-default-features
cargo test --locked -p trollm-uniffi --test integration --features stub
cargo test --locked -p trollm-models --features llama --lib
```

The first two Rust checks use the real gateway with scripted models and install
the actual bundle without inference. The native grammar check uses a generated
tiny vocabulary, with no model weights. These checks cover wiring, argument
binding and grammar acceptance; real-model operand selection and native-device
execution need separate acceptance on an APK built from this source.

## Temporary implementation seam

The production gateway implements this contract. For a downstream branch that
has not yet integrated it, a temporary test gateway can recognize
`local.calculate` and implement this exact contract, delegating other names to
its existing MCP gateway. Keep that code outside the core executor. Use the
shared JSON cases for positive results and failure behavior; reject unsupported
inputs rather than return a canned success. Fixture-only gateways must be marked
as scripted evidence and cannot stand in for real-model/device acceptance.

The replacement seam is `ToolGateway`: catalog, argument validation, read-class
classification, execution and normal artifact/result handling. A temporary
adapter must retain all five responsibilities. Production integration should
remove the temporary implementation; flow tool names, arguments and bindings
remain unchanged. No Amazon-specific task names, selectors or prose parsing
belong in that seam.
