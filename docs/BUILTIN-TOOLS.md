# Built-in tools: interface contract v1

This contract covers all nine implemented local tools:

| Tools | Contract |
| --- | --- |
| `local.calculate` | [Exact decimal arithmetic and comparison](#calculator-contract) |
| `local.assert_equal` | [Structural JSON equality](#structural-json-equality-contract) |
| `local.select_unique` | [Unique original-object selection](#unique-object-selection-contract) |
| `local.prepare_worklist` | [Prepared input and occurrence identities](#prepared-worklist-contract) |
| `local.collection_total` | [Complete collection coverage and sum](#complete-collection-total-contract) |
| `local.extract_decimal` | [Literal decimal extraction](#literal-decimal-extraction-contract) |
| `local.filter_records`, `local.select_matching`, `local.order_records` | [Structural record operations](#structural-record-operations) |

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

## Structural JSON equality contract

`local.assert_equal` is an explicitly granted, coder-invoked local read tool.
Supply exactly `left` and `right`, both required and each any parsed JSON value.
It compares the two `serde_json::Value` values directly: object key order is
ignored, array order matters, strings are literal and case-sensitive, and no
decoding, trimming, numeric coercion or Unicode normalization occurs. A missing
operand is invalid; two explicit null operands are equal.

Equality follows the parsed number representation: integer `1` differs from
floating `1.0`, and integer `0` differs from `0.0`; floating `1.0` and `1e0`
are equal, as are floating `0.0` and `-0.0`. This is equality of parsed values,
not a comparison of the original JSON spelling or exact-decimal arithmetic.

Success returns exactly `{"equal":true}` through the normal result policy and
artifact path. Mismatch returns `local.assert_equal: mismatch` without operand
values or a successful proof artifact. Standalone malformed requests return
`local.assert_equal: invalid-arguments`; gateway schema or byte/depth admission
may instead return the existing bounded diagnostic. The standard combined
256 KiB argument-JSON and depth-64 limits remain unchanged. The contracts are
[input](../schemas/local-assert-equal.v1.input.schema.json) and
[output](../schemas/local-assert-equal.v1.output.schema.json).

Authored flows can project the fields whose structural equality matters into
separate views, preserve the original artifacts, and bind both operands with
`modelFields: []`. The coder still invokes an ordinary tool call; a later task
uses the original evidence and the proof artifact. Equality proves only literal
agreement of those operands, not freshness, ownership or semantic suitability.

## Unique object selection contract

`local.select_unique` is an explicitly granted, coder-invoked read tool. It uses
the existing local dispatch, argument bindings, result validation, artifacts and
response-size telemetry. It requires no connection and has no browser, DOM,
selector, URL or site semantics. Its schemas are [input](../schemas/local-select-unique.v1.input.schema.json)
and [output](../schemas/local-select-unique.v1.output.schema.json).

Supply the required `items` (an array of original objects), `field` (a JSON pointer),
`equals` (a literal string) and `prefix` (a literal string, possibly empty).
An item matches precisely when `prefix + item[field]` equals `equals`, with
case-sensitive string comparison. Missing or non-string field values do not
match. JSON pointer escapes `~0` and `~1` address literal tilde and slash keys;
malformed pointers fail. Empty pointer addresses the object itself and therefore
does not match a string. There is no URI-fragment or percent decoding, Unicode
normalization, trimming, regular expression evaluation or fallback selection.

Exactly one match returns `{selected, index, sourceSha256}`. `selected` is the
unchanged original object, including fields not used by the predicate; `index`
is its zero-based position in the complete input array. `sourceSha256` hashes
the entire bound `items` JSON value using compact `serde_json` serialization
(the same bytes as `serde_json::to_vec(items)`), preserving array order. It
identifies the supplied value, not the original artifact's whitespace or bytes.
Preserve that original artifact separately. Reordering unique candidates keeps
the selected object but changes its index and source hash.

Zero matches fail with `local.select_unique: no-match`; multiple matches fail
with `local.select_unique: ambiguous`, including identical duplicate objects.
Neither failure publishes a selected result artifact. Invalid standalone
arguments use `local.select_unique: invalid-arguments`; gateway schema and
byte/depth admission may report their existing bounded diagnostics instead.

Selection admits at most 100,000 objects and 32 MiB of compact serialized items,
matching the existing complete-query collection allowance. Total argument JSON
is bounded to 33 MiB and depth 64. The three structural record tools below share
that argument-byte allowance; other tools retain their 256 KiB bound. `field` is bounded
to 4,096 UTF-8 bytes, and `equals` and `prefix` to 16,384 bytes each. Bounds are
checked before matching/hash work, and oversized inputs fail without truncation.
Flows may apply smaller result/artifact budgets; complete sources must fit those
budgets too. A selected object is never silently shortened to make a result fit.

An authored task can bind all four arguments and use `modelFields: []`; its
coder still invokes the ordinary tool-call fence. A later ordinary tool step
can bind `selection.outputs.data.selected.id` without transcribing an ID from
model prose. This operation establishes literal uniqueness only, not semantic
suitability or freshness of the selected object.

`local.select_unique` also accepts optional `matchMode: array-contains` for a
JSON-pointer field containing an array of strings, such as original affected
record IDs in an action result. It uses literal equality with the same prefix
rule and selects exactly one original object. Duplicate matching strings within
one object still identify one object; two matching objects are ambiguous. Mixed
type arrays do not match. Omission or `equals` preserves the original scalar
contract. This is membership checking, not interpretation of adviser text.


### Explicit literal text lookup

The opt-in matchMode: string-contains searches a nonempty literal substring in declared string fields. `field` may be one JSON pointer or up to 32 pointers; a record matching several fields counts once. Missing or non-string fields do not match. Zero matches and multiple original records fail without a selected result. Returned records, indices and complete-source hashes are unchanged. Default equality and array membership retain their original contracts.

Optional stopBefore uses the portion of equals before the first literal marker, after existing prefix removal; without a marker occurrence it uses all of the supplied string. Empty markers or empty resulting search strings are rejected. This option is allowed only for string-contains. There is no trimming, case folding, HTML decoding, regex evaluation, ID inference or modification of source records. Fields remain explicit authored data. Lists of fields are allowed only in string-contains mode.

This is evidence retrieval, not semantic approval or freshness. A matching record still needs the flow's identity/ownership and fresh-observation checks before any action.

## Prepared worklist contract

`local.prepare_worklist` accepts exactly `{input: string}`. The string contains
one JSON envelope with exactly `context` (a string, possibly empty) and `items`
(an array of 1–8 original JSON objects). The tool has no item-domain rules;
request prose, quantity or ticket-specific constraints belong in the flow's
result schema. Its contracts are [input](../schemas/local-prepare-worklist.v1.input.schema.json)
and [output](../schemas/local-prepare-worklist.v1.output.schema.json).

Admission rejects malformed, empty or oversized input without partial results.
The original input string is at most 48 KiB of UTF-8; decoded context is at most
4 KiB of UTF-8; each compact serialized item is at most 4 KiB. The envelope has
a maximum nesting depth of 32. Combined argument JSON retains the standard
256 KiB/depth-64 bound. Schema string lengths count characters; the tool and
gateway additionally enforce byte bounds and the decoded envelope contract.

Success returns exactly `{format, context, items}`, with format
`trollm.local.prepare_worklist.v1`. Each output item is `{id, value}`, preserving
the original parsed object unchanged. No truncation, normalization, inference,
sorting or duplicate consolidation occurs. Duplicate original entries receive
different occurrence IDs. IDs are lowercase SHA-256 hex of the concatenation
of the UTF-8 domain string `trollm.local.prepare_worklist.v1`, a zero byte, the
complete original input string's UTF-8 bytes, and the zero-based occurrence
position as an unsigned 64-bit big-endian integer. Identical input/position is
deterministic; a whitespace or context change also changes IDs. IDs identify
these occurrences, not semantic equivalence or the freshness of evidence.
Preserve the original input and original evidence separately.

Standalone malformed requests use `local.prepare_worklist: invalid-arguments`;
size/depth or gateway schema admission may instead use bounded diagnostics.
The fully bound coder call can use `modelFields: []`; the tool never creates a
model-authored worklist or a new executor node kind.

## Complete collection total contract

`local.collection_total` accepts exactly `expected`, `results`, `expectedId`,
`resultId`, `outcome` and `amount`. Both arrays contain 1–8 values. The remaining
four arguments are static RFC 6901 JSON pointers into each expected value or
result value, respectively. Empty pointer addresses the whole value; slash and
tilde keys use `~1` and `~0`. No fragment decoding or expression evaluation occurs.
Pointer strings and selected nonempty identities are bounded to 4,096 UTF-8
bytes. Combined argument JSON is at most 256 KiB and depth 64, checked by the
tool itself as well as gateway admission. Bind compact result records rather
than repeating full observations. Contracts are [input](../schemas/local-collection-total.v1.input.schema.json)
and [output](../schemas/local-collection-total.v1.output.schema.json).

Every expected identity must occur exactly once on each side. Duplicates,
missing or unexpected results fail. Every selected outcome must be the literal
string `succeeded`; failed, skipped or missing outcomes cannot be omitted from
the sum. Each selected amount must be a calculator-format decimal string, with
the same 12 integer/6 fractional digit limits. Addition is exact with no
rounding; the final magnitude is bounded to `999999999999.999999`.

Success returns exactly `{format, count, sum, terms}`, with format
`trollm.local.collection_total.v1`. The sum is canonical decimal text. Terms
are `{id, amount}` in expected-array order and retain the exact original amount
strings, including trailing zeros. This establishes identity coverage and exact
arithmetic only; the authored flow still checks currency, semantic identity,
price basis, and whether a claimed successful outcome is supported by evidence.

Stable failures use the `local.collection_total:` prefix with
`invalid-arguments`, `duplicate-expected`, `duplicate-result`,
`coverage-mismatch`, `unsuccessful-result` or `out-of-range`. Size/depth and
gateway schema admission may report bounded diagnostics instead. No failure
publishes a successful result artifact. Both collection tools use explicit task
grants and the existing local read-tool policy, binding and artifact pipeline.
Focused fixture/gateway coverage is in `worklist_collection` (no real models,
phone execution or live MCP).

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

Built-in local tools are read-class operations. Their policies support the existing
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
The [direct comparison task](DIRECT-TOOL-EXECUTION.md) fixes `operation: compare`,
binds the saved sum via `fromVars` and reads the budget via `fromInput`.
Both exact payloads and the original input reach the tool-free reasoner and
final coder. The coder also receives the reasoner's advice.
No calculation or model prose authorizes an external action.

The sum prompt explicitly teaches the `<tool_call>` envelope. The runtime constrains
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

## Structural record operations

`local.filter_records`, `local.select_matching` and `local.order_records` are
explicitly granted coder-invoked read tools. Their versioned contracts are:

| Tool | Arguments | Result |
| --- | --- | --- |
| `filter_records` | [Input schema](../schemas/local-filter-records.v1.input.schema.json) | [Output schema](../schemas/local-filter-records.v1.output.schema.json) |
| `select_matching` | [Input schema](../schemas/local-select-matching.v1.input.schema.json) | [Output schema](../schemas/local-select-matching.v1.output.schema.json) |
| `order_records` | [Input schema](../schemas/local-order-records.v1.input.schema.json) | [Output schema](../schemas/local-order-records.v1.output.schema.json) |

They accept original object arrays (at most 10,000 records, 32 MiB compact JSON,
64 levels), combined arguments of at most 33 MiB/depth 64, and an offline JSON
Schema predicate (16 KiB, depth 16), using the
existing bounded validator. No external schema fetches, expressions, browser
code or business-specific matching are provided. Bind the predicate from authored
data; it is a structural filter, not a semantic product/ticket decision.

- `filter_records({items, schema})` returns `matches`, each containing the original
  `index` and unchanged `selected` object, plus `matchedCount`, `excludedCount`
  and `sourceSha256` of the complete compact original array. Order is preserved;
  zero matches is a valid result. Require a nonempty output explicitly when needed.
- `select_matching({items, schema})` requires exactly one match and returns
  `index`, `selected`, `sourceSha256`. Absence and duplicates fail without a
  selected artifact. It does not choose between multiple structurally valid options.
- `order_records({items, schema, field, prefix, fractionDigits, groupSize})`
  orders admitted records by a literal decimal string at the JSON pointer `field`.
  The prefix is exact, the entire remaining string must be a supported decimal,
  and its fractional digits must equal `fractionDigits` (0..6). Trailing prose,
  separators and excess digits fail; they are never trimmed or guessed. Sorting
  is ascending and stable for ties. Each result retains `index`, `selected` and
  canonical `decimal`. `groups` contains sequential `{index, items}` batches
  of 1..32 records, alongside the same counts and original source hash.

The ordering field is a JSON pointer of at most 4,096 UTF-8 bytes; its literal
prefix is at most 256 bytes. Decimals use the calculator's 12-integer-digit and
six-fractional-digit bounds. An empty admitted set produces empty `matches` or
`groups`; unique selection instead reports `no-match`. One admitted record with
a missing field or invalid decimal fails the entire ordering operation. It does
not silently drop that record or return a partially sorted list.

Ordering establishes numeric order only. A price label might still be a discount,
unit price or unrelated product; the flow/model must establish its meaning.
Configure `resultArtifact` on each local policy to retain complete results, and
project only the prompt view. Schema/invocation errors fail; there is no truncation
or fallback to an arbitrary first match. Existing gateway grants, argument
validation, artifacts and response-size telemetry apply.

Validation: `cargo test --locked -p trollm-mcp --test record_tools`.

### Optional literal ordering-key window

`local.order_records` additionally accepts optional `keyExtraction` with required
`stopBefore` (at most 16 nonempty literal markers, 256 UTF-8 bytes each) and
`repeated: identical`. This requires a nonempty prefix. The numeric window ends
before the earliest exact, case-sensitive marker and is trimmed. It must consist
only of one or more prefixed decimal strings, separated by optional whitespace.
Every parsed amount must be identical. Distinct amounts, arbitrary prose, missing
values and malformed decimals fail the entire operation, without partial output.
For example, an authored `stopBefore: ["("]` admits `$6.97$6.97 ($0.56/unit)` as
key `6.97`. It does not establish that `6.97` is a retail price. The model/flow still
checks semantic price role and source ownership. Original objects and source hash
are preserved. Omitting this option preserves the previous strict whole-field
contract. Installing a flow with this option requires an updated native APK.

### Choosing between literal identity and semantic judgment

The [readable-record example](../flows/examples/record-lookup.flow.yaml) uses the
text lookup mode to resolve a coder's observed phrase to an unchanged original.
Do not use a phrase match to infer product suitability, ticket priority or source
freshness. The selected artifact proves unique literal membership only. A delimiter
such as a comma is an authored boundary, never a general rule for removing irrelevant
text. Keep ambiguity and no-match paths available.

Likewise, an ordering key extracted from repeated identical decimals establishes
numeric order for the declared text window. It does not identify a price's role,
convert units or verify a budget forecast. Tests include a non-currency score label
and reject conflicting repetitions without partial sorting. All new options are
explicit; older default operations keep their existing contracts. The schema,
fixed-argument validator and full bound gateway call must agree on these options.
