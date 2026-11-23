# Security policy

## Reporting a vulnerability

Please do not post vulnerability details, credentials, private flow inputs or
unsanitized logs in public Issues or pull requests.

Use GitHub's private
[Report a vulnerability](https://github.com/e2llm/trollm/security/advisories/new)
button under **Security → Advisories** when available.

If the button is unavailable, ask the repository owner to enable private
reporting, without including vulnerability details in that request. Once the
channel is available, include the affected source/build identity, impact and a
small reproduction with secrets and personal data removed. There is no promised
response deadline.

## Supported scope

Security fixes target the latest candidate and current source. This prototype
has no long-term support commitment for older APKs. Report the source commit,
APK version and checksum when available; a source checkpoint does not establish
acceptance of a matching APK.

## Viewer dependency checks

The viewer requires zero findings in the full and production npm audits.
GitHub withdrew the Picomatch advisory on November 18, 2026, and the temporary
development-tool exception was retired. [Advisory and audit policy history](docs/SECURITY-EXCEPTIONS.md)

GitHub's **Security → Dependabot alerts** is the maintainer dependency-alert
surface. [Tracking issue #7](https://github.com/e2llm/trollm/issues/7)

## Trust and data handling

Install and run flows only from authors you trust. Bundles are not publisher
signed: their hashes verify consistency with the included manifest, not who
created it. A flow author controls prompts, tool declarations, policies, MCP
endpoints and model download setup. A malicious flow can request data from
connected services, send inputs or tool results to remote services, or perform
mutations using the access you have granted. Authored tool policies are not an
independent certification that a flow is safe.

Model inference runs on the device, but MCP calls, authentication and model
downloads use the network. Local run history, tool arguments/results and event
logs may contain sensitive data; review and sanitize them before sharing.
Credentials belong in connection settings, never in a bundle or bug report.
Endpoint-bound credential checks and integrity validation reduce specific risks;
they do not make an untrusted flow safe to run.

See the [security design and deferred work](docs/SECURITY-ENTERPRISE.md) for the
current protections and their limits.
