# Flow review companion

Start here for the self-hostable human review companion to LLM-authored trollm
flows. The owner approved this work on 2026-10-24. The companion is implemented
and covered by host, browser and actual Podman checks. The implementation record
below describes the validation scope and delivery status.

1. [Design](DESIGN.md): product scope, code boundaries, Podman delivery and safety.
2. [Interface](INTERFACE.md): presentation file and server/browser contract.
3. [Implementation record](IMPLEMENTATION.md): completed scope, validation limits
   and guidance for continuing development.

The application lives in `apps/flow-review/`. Its Rust server participates in the
root workspace but is not a dependency of Android or the executor. The browser
has its own package/lockfile. Source flows remain authored by an external LLM;
humans inspect, comment, export feedback, and compare the next revision.

These documents describe an approved companion application, not a new execution
plan format. Existing plan, registry and runtime contracts remain authoritative.
