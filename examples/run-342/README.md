# Two recorded v342 runs

These are two completed executions of an authored shopping flow using small
local language models on Android phones. They are interesting examples to inspect
and investigate further. They are selected runs, not a reliability measurement.

The mission was to put two retail units of one Head & Shoulders shampoo SKU and
one yellow rubber duck retail package in an initially empty Amazon cart, with a
merchandise subtotal strictly below USD 20. The duck package's internal piece
count was unrestricted. Shipping and taxes were outside the supplied merchandise
budget. Neither run attempted checkout or payment.

| Recorded run | Duration | Coder / reasoner calls | Tool calls | Final merchandise subtotal |
| --- | --- | --- | --- | --- |
| [Samsung A04e](a04e/README.md) | 4h 20m 21s | 550 / 28 | 537 | USD 13.69 |
| [Samsung S21](s21/README.md) | 1h 41m 31s | 534 / 28 | 522 | USD 18.53 |

Both records describe one Start from an independently checked empty cart, with
no Resume, seeded state, skipped stages or manual browser assistance during
execution. Both end with native `succeeded` and record a separate final-cart
check after completion. Authored recovery handled four and five tool failures,
respectively. Other attempts in development stopped or remained unresolved; this
pair was selected because both completed. The products and observed paths differ,
so the timing difference is not an isolated hardware benchmark.

## Small contexts over a long procedure

The models were Qwen3.5-0.8B Q4_K_M in the tool-capable coder role and
Qwen3.5-2B Q3_K_M in the tool-free reasoner role. Both had an 8,192-token context,
temperature 0.1, top-p 0.9 and thinking requested off. [Model pins](models.json)
identify the exact files and download revisions; weights are not included.

| Recorded input tokens per model call | A04e | S21 |
| --- | --- | --- |
| Median, all calls | 45 | 45 |
| 95th percentile, all calls | 229 | 211 |
| Median, reasoner calls | 378 | 418.5 |
| Maximum, either role | 1,237 | 1,253 |

These are native tokenized, chat-templated inputs, including applicable model
control tokens. They are not the allocated context capacity. Many coder calls
invoke a single tool with arguments already supplied by bindings, which explains
the small overall median. The runtime carries saved state and original evidence;
each model call receives the inputs for its authored task. The recorded model
call totals are about 251 minutes on A04e and 95 minutes on S21. Measurement
scopes overlap; adding all timing fields does not reconstruct wall time.

## What is included

- [Exact installable v342 bundle](amazon-cart-v342.tar), shared by both runs.
- [Readable flow source](flow/amazon-cart.flow.yaml), [task registry](flow/amazon-cart.tasks.yaml),
  prompts and child flows extracted unchanged from that bundle.
- Each run's result, historical identities, metrics, derived context statistics,
  terminal observation and evidence archive.
- [Provenance](provenance.json) and [publication checksums](SHA256SUMS).

The historical archives were already sanitized before this publication. Most
page text, identifiers and model prose are represented by redaction markers with
their original byte counts and hashes. Public product names and final result
details remain in the result files. This preserves useful execution and timing
evidence, but does not let a reader independently reassess every semantic choice.
No page or model text has been reconstructed from hashes. The metrics reader
recognizes the hashes of the two public runtime phase labels `attempt` and
`tool_call`, which the historical sanitizer also redacted. Public product text and model
instructions do not need blanket redaction in future examples; credentials and
incidental account or personal data still need review.

The archives retain their existing bytes. Their original-file manifests describe
files **before** redaction; use `SHA256SUMS` to verify the published files. The
historical signer differs from the 1.0.0 release signer. These runs used
APK68 / 0.1.0-rc.24 with an imported v342 flow, not the 1.0.0 release APK. Exact
source, APK, bundle and signer hashes remain in the identity records.

## Inspect or try it

Run `python3 scripts/check-run-examples.py` from the repository root to verify
the file checksums, bundle contents and recorded aggregate measurements.
To inspect one archive and recompute its measurements:

```sh
python3 -m zipfile -e examples/run-342/a04e/run-evidence.zip /tmp/trollm-a04e-v342
python3 scripts/run_metrics.py \
  /tmp/trollm-a04e-v342/abec4388-db0b-4276-8f55-6e91373cda09/events.jsonl \
  /tmp/trollm-a04e-v342/abec4388-db0b-4276-8f55-6e91373cda09/proxy.events.jsonl
```

The [Android installation guide](../../docs/INSTALL-ANDROID.md) links the release
APK and explains model preparation. Import the v342 bundle using **Install
bundle**. This browser flow additionally needs an E2LLM-connected browser and
its authenticated MCP connection. Supply that browser's name/ID, a prepared
mission in the format shown by the flow setup, and a separate decimal budget.
The procedure expects exactly one browser tab: the intended Amazon tab.
It modifies the cart and may stop after partial changes; the authored procedure
does not perform checkout. The website and service can change after these runs.

The archived journals are evidence for inspection, not replay instructions.
