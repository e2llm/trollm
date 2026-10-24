# trollm — Enterprise Security & Auditability (tech-debt register)

**Context**: target deployments include on-prem, air-gapped, enterprise environments.
Security and auditability are hard requirements; parts are deferred by design and tracked
here. Nothing in v1 may foreclose anything on this list.

For vulnerability reporting and supported versions, see the
[security policy](../SECURITY.md). Run only flows from trusted authors: unsigned
bundles can declare remote endpoints and tool actions that read, transmit or
change data using granted service access. Bundle hashes do not authenticate
publishers. Local logs and run history may contain tool arguments and results;
sanitize them before sharing. Bounded model-failure artifacts can additionally
contain authored prompts, bound observations and partial model responses. These
stay in the run's artifact directory (mode 0600 on Unix) and are removed with the
run. They are not automatically uploaded; manually copying them can disclose
sensitive data.
Direct core host APIs read authored prompt paths with the host user's filesystem
authority; Android bundle imports additionally require verified, contained
manifest paths. These are different trust boundaries.

## Designed-in now (non-negotiable invariants that security builds on)

- **Content integrity**: bundle members and model weights are SHA256-verified;
  raw flow bytes determine flow identity. Optional `sourceBaseline` is descriptive
  provenance, not a publisher signature or an independently enforced trust anchor.
- **Append-only event log**: core records transitions, tool results, declared
  mutation intents and artifact writes. Proxy decisions use a separate
  sidecar; write/reopen errors fail closed.
- **Deterministic control flow**: transitions never depend on model prose → audit can
  reconstruct *why* a run took a path from the log alone.
- **Mutation journaling**: intent-before-execute is required for policy-classed
  MCP mutations, including nested flows. Unresolved or unsafe completed writes
  fail closed on resume; automatic retries are rejected for mutation-grant and
  intent-log tasks. Real state reconciliation is not implemented. Classification
  depends on authored policies, and process-crash checks do not establish
  power-loss durability.
- **MCP egress control**: all MCP traffic passes the in-process gateway. OAuth and
  model downloads use separate network clients.
- **TLS-only** to plan-declared hosts; tokens in Android Keystore-backed storage.
- **Endpoint-bound credentials**: manual secrets are retrieved only for their
  explicitly saved canonical HTTPS endpoint. Legacy name-only secrets require
  re-save; OAuth retains its separate grant/endpoint binding.
- **No remote telemetry or analytics.** Numeric size/deadline diagnostics stay
  in the local run journals; no crash-reporting or automatic diagnostic uploads.

## Deferred (tech debt) — ordered by priority

| # | Item | Notes / acceptance shape |
|---|---|---|
| 1 | **Signed content bundles** | Flows, registries, prompts, model manifests signed (ed25519 or enterprise PKI); app verifies before install. `.sig` files already reserved in storage layout. |
| 2 | **Hash-chained event log** | Each line includes `prevHash`; run sealed with a final digest → tamper-evident audit trail, exportable to SIEM. |
| 3 | **mTLS + internal CA** | Client certs for MCP egress; cert pinning per host; airgap CA provisioning story. |
| 4 | **Egress allowlist enforcement** | Network security config + gateway-level host allowlist from enterprise policy, not just plan. |
| 5 | **RBAC on flow installation** | Enterprise decides who may install/run which plans; device-local policy file, signed. |
| 6 | **Airgap update channel** | Signed bundles via USB/ADB/MDM side-load; version pinning + rollback. |
| 7 | **SBOM + dependency policy** | cargo-sbom/cargo-audit in CI; Gradle equivalent; the dependency allowlist in ARCHITECTURE-ANDROID.md §13 exists partly for this. |
| 8 | **Secrets beyond bearer tokens** | HSM/Keystore-attested keys; rotation; per-run credential scoping. |
| 9 | **PII/data-classification hooks in `shape` policies** | Strip/redact classes of fields before they reach model context; policy-driven. |
| 10 | **Multi-device audit correlation** | When dora/Zenoh multi-device lands: correlated run ids across devices, central (on-prem) log aggregation. |

## Explicit non-goals for v1 (documented so nobody "helpfully" adds them)

- No cloud sync, no analytics, no crash-reporting SDKs.
- No remote flow execution or remote model calls.
- No dynamic plugin loading on-device.
