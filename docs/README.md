# Documentation

Start with the [project overview](../README.md), then
[Anatomy of a flow](ANATOMY-OF-A-FLOW.md) and its small source example.
[Agent onboarding](../AGENTS.md) gives an ordered reading path and code map;
[Contributing](../CONTRIBUTING.md) explains how to check and submit changes.

| Goal | Read |
| --- | --- |
| Understand the roles and design limits | [Premise](PREMISE.md), [current architecture](ARCHITECTURE-ANDROID.md) |
| Author or revise a current flow | [Flow sources](../flows/README.md), [small-model practices](FLOW-PRACTICES.md), [routing and step context](ADAPTIVE-FLOWS.md), [built-in tools](BUILTIN-TOOLS.md) |
| Inspect a large package and send feedback to its authoring LLM | [Flow review companion](../apps/flow-review/README.md), [design and interface](flow-review/README.md) |
| Work on Android setup, models or execution | [Android guide](../apps/android/README.md), [architecture](ARCHITECTURE-ANDROID.md) |
| Find priorities and remaining acceptance work | [Product plan](PRODUCT-PLAN.md) |
| Understand proposed format work | [Flow contract direction](FLOW-CONTRACT-DIRECTION.md), [authoring direction](FLOW-AUTHORING.md); proposed fields are not current contracts |
| Prepare source or APK releases | [Versioning](VERSIONING.md), [third-party notices](../THIRD_PARTY_NOTICES.md) |
| Report or assess security issues | [Security policy](../SECURITY.md), [enterprise security](SECURITY-ENTERPRISE.md) |

The schemas, Rust readers and architecture implementation qualifications define
the current contract. Host/scripted tests establish wiring and recovery rules;
they do not establish small-model accuracy or acceptance on a physical phone.
