# Security exception history

## Picomatch development tooling

GitHub withdrew GHSA-fc5q-39xc-vjpf / CVE-2026-112366 on November 18, 2026,
after determining that the report did not describe a security issue.
The maintainer had approved a scoped development-tool exception on
November 17. That exception is retired. [Withdrawn GitHub advisory](https://github.com/advisories/GHSA-fc5q-39xc-vjpf)
[Tracking issue #7](https://github.com/e2llm/trollm/issues/7)

The viewer requires zero findings in both the full and production npm audits.
Any reported finding blocks the gate, including a recurrence of the withdrawn
advisory. The dependency lockfile continues to pin Picomatch 4.0.7. New
publication candidates require the strict audit receipt and a verified inventory
of actual bundled modules and asset hashes. The viewer build still excludes
development glob tooling from its served assets. [Audit policy](../apps/flow-review/web/scripts/audit-policy.json)

Images qualified under the former exception require a fresh build, audit and
scan under the strict policy before any new publication. Use a new source
revision and its own immutable image tag. The bytes and evidence of already
published images remain associated with their original revisions.
