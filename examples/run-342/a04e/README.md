# v342 on Samsung Galaxy A04e SM-A042F

This is one recorded run on November 13, 2026, published for inspection.
Read the [shared description](../README.md) for the mission, model settings,
measurement definitions and the limits of the available redacted evidence.

- Run: `abec4388-db0b-4276-8f55-6e91373cda09`.
- Recorded native outcome: `succeeded`.
- Journal duration: 15,621.288 seconds.
- Reported final merchandise subtotal: USD 13.69, strictly below USD 20.
- Final recorded quantities: two shampoo retail units and one yellow duck retail package.
- One Start, no Resume, seeded evidence, skipped stages or manual browser assistance during execution.
- The run record reports an independent final-cart check after completion; no checkout occurred.

[Result](result.json) records the selected products and quantities.
[Identities](identities.json) identifies the historical APK and flow.
[Metrics](metrics.json) contains the recorded aggregate measurements;
[context statistics](context-stats.json) are derived from the archived journals.
[Run evidence](run-evidence.zip) preserves the already-sanitized historical archive
byte for byte. [Original-file metadata](original-file-manifest.json) records the
pre-redaction hashes and sizes supplied with that archive; those hashes are not
checksums of the redacted files. The publication checksums are in
[SHA256SUMS](../SHA256SUMS).

The [terminal observation](terminal.json) may retain an earlier recovered
`lastError`. Its final `status` and the journal's `run_ended` record identify
the terminal outcome. Timing scopes overlap and must not be added into wall time.
