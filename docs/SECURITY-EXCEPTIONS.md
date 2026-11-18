# Time-limited security acceptance records

An acceptance records a reviewed, bounded risk. It does not fix the upstream
package or waive unrelated findings. Expiry does not renew automatically.

## picomatch development-tool ReDoS

**Approved by the maintainer:** 2026-11-17, for the scope below.
**Expires:** 2026-11-24T00:00:00Z.
**Upstream:** [GHSA-fc5q-39xc-vjpf / CVE-2026-112366](https://github.com/advisories/GHSA-fc5q-39xc-vjpf).

At approval, upstream identifies versions through 4.0.7 as affected and lists no
patched version. The reported impact is denial of service: attacker-controlled
glob patterns can cause excessive regular-expression work and block a Node.js
process. The acceptance is limited to build, test and development tooling for
the flow-review companion.

### Exact accepted scope

- Package: `picomatch`, exactly **4.0.7**, high severity for this one advisory.
- Manifest: `apps/flow-review/web/package-lock.json`.
- Approved lockfile SHA-256:
  `7070eafefedc72065b039f169455c950cdfe695c783aa4c5154a960ac1de40e7`.
- Exactly these development-only installations:
  `node_modules/tinyglobby/node_modules/picomatch`,
  `node_modules/vite/node_modules/picomatch`, and
  `node_modules/vitest/node_modules/picomatch`.

The [machine-readable policy](../apps/flow-review/web/scripts/audit-exceptions.json)
and [portable audit gate](../apps/flow-review/web/scripts/audit-gate.mjs) enforce
these limits. Transitive affected packages qualify only when their complete
advisory chains lead exclusively to this accepted advisory and remain in the
approved development dependency graph. This is not a package-name allowlist.

### Exposure and remaining risk

The production dependency audit at approval reports zero vulnerabilities.
An actual Vite output-module inventory contains 19 npm packages and excludes
picomatch, micromatch, minimatch, tinyglobby and fdir. The viewer's uploaded flow
files are inspected by Rust; they are not passed to a Node glob API. The provided
Containerfile uses Node in its build stage and serves the built assets with Rust
in the final stage. The Android app does not use this Node toolchain at runtime.

The development tooling still contains vulnerable code. A hostile pattern
reaching it through configuration, plugins or a changed integration could hang
a build or development process and consume resources. Checked-in patterns are
currently fixed repository configuration. Do not expose the Vite development
server as the production viewer or add untrusted pattern input to these tools.
Different images, build arguments or deployments need their own verification.

### Checks that remain mandatory

1. Run the full npm audit before build/test tools execute. Any other high or
   critical advisory, critical severity for this advisory, unexpected report
   shape, changed version/path/lockfile or expired acceptance blocks the job.
2. Require a separate zero-finding production audit while using the exception.
3. Verify a fresh inventory of actual built modules and output hashes. Missing,
   stale or mismatched evidence, or inclusion of the affected glob tooling in
   the delivered viewer assets, blocks the final gate.
4. Retain the full audit reports. Report this advisory as accepted temporarily;
   do not present the underlying `npm audit --audit-level=high` as passing.

The independent minimatch advisory was resolved by updating the existing
development dependency `typescript-eslint` to 8.73.0, which resolves patched
minimatch 11.0.9. That change is separate from this acceptance. Forced Vite or
Vitest downgrades are not treated as evidence of a picomatch fix.

### Removal and GitHub tracking

Remove the acceptance earlier when a compatible, released upstream fix is
available. Update the lockfile and rerun the full/production audits, web tests,
production build, bundle inventory and image checks. The upstream
[release list](https://github.com/micromatch/picomatch/releases) and advisory
identify published fixes; npm's suggested dependency downgrade alone does not.

Use [GitHub Dependabot alerts](https://github.com/e2llm/trollm/security/dependabot)
for maintainer dependency findings and [tracking issue #7](https://github.com/e2llm/trollm/issues/7)
for this dated decision.
Leave any matching alert open until remediation is verified: GitHub's
`tolerable_risk` dismissal has no documented automatic expiry. This record is
not a new vulnerability advisory for trollm, and it does not apply to older
lockfiles containing additional picomatch versions or paths.

The GHCR workflow also checks the absolute expiry before publication. A failed
publication may retry the original validated/scanned archive without rebuilding
or fabricating fresh evidence, but only before that expiry. Build-time freshness
checks remain mandatory; the immutable source tag cannot be replaced with a
different image digest.
