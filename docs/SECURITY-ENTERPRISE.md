# trollm — Enterprise Security & Auditability (tech-debt register)

**Context**: target deployments include on-prem, air-gapped, enterprise environments.
Security and auditability are hard requirements; parts are deferred by design and tracked
here. Nothing in v1 may foreclose anything on this list.

## Designed-in now (non-negotiable invariants that security builds on)

- **Content-addressed everything**: flows, task registries, model weights, artifacts are
  sha256-pinned; plans pin `sourceBaseline`. (Foundation for signing and attestation.)
- **Append-only event log**: every transition, tool call (intent + result), artifact write,
  and proxy decision is logged. (Foundation for audit.)
- **Deterministic control flow**: transitions never depend on model prose → audit can
  reconstruct *why* a run took a path from the log alone.
- **Mutation journaling**: intent-before-execute for all MCP writes, with reconcile-on-resume.
  (Foundation for "who/what/when wrote to the ticket system" accountability.)
- **Single egress choke point**: all MCP traffic passes the in-process gateway; no other
  network path exists in the architecture.
- **TLS-only** to plan-declared hosts; tokens in Android Keystore-backed storage.
- **No telemetry.** Anywhere. Ever.

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
