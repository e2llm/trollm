# Native collection member offer

This is an optional integration for a producer that supplies an owned collection
and its execution authority. The ordinary Android starters and the
[v342 examples](../examples/run-342/README.md) do not require this integration.

`collection-members` builds the existing `ledger.execution-plan.v1`,
`ledger.task-registry.v1`, `trollm.flow-setup.v1` and `trollm.flow-bundle.v1`
members. Building members does not publish a source, install an application,
obtain credentials, start a worker or dispatch an effect.

The separate source and member profiles use the proposal identifiers
`moray.declared-native-collection.v1-proposal` and
`trollm.native-collection-members.v1-proposal`. They do not replace the current
text-only source decoder or the normal flow bundle format. See the
[source connector](MORAY-SOURCE-CONNECTOR.md) for ownership boundaries.

## Builder and inputs

With the pinned Rust toolchain, the builder accepts bounded regular files and
an explicit endpoint. For example, replacing all placeholder paths and endpoint:

```sh
cargo run --locked -p trollm-uniffi --bin collection-members -- \
  /absolute/selection.json /absolute/selection-ref.json /absolute/owner.json \
  /absolute/context.txt https://mcp.example.com/mcp /absolute/new-members
```

The selected bytes, raw hashes, owner, input ordering and context must agree.
The builder writes a new directory and refuses to replace an existing output.
Its output describes the compiled members and their pins; it does not establish
installed runtime support or a completed business outcome.

The generated package retains the selection and context, prepares a worklist
from the original records, iterates the supplied items and uses coder-invoked
tools for each authored operation. Values already present in original evidence
move through bindings. A model cannot manufacture source membership or authority
by returning a matching label or a valid JSON envelope.

The exact current data contracts are the
[selection schema](../schemas/declared-collection-selection.v1.schema.json),
[source schema](../schemas/declared-native-collection-source.v1.schema.json),
[member builder](../crates/trollm-uniffi/src/collection_members.rs) and
[provider](../crates/trollm-uniffi/src/collection_provider.rs), with their tests.
Compiled members, installed source admission, native execution and externally
observed outcomes remain distinct records.

## Preparation, dispatch and retained results

The native owner installs the preparation provider and its association with the
selected input. Preparation can supply bounded original evidence and bound
arguments. It does not authorize a send by itself. The runtime validates the
prepared call, retains its preparation and mutation intent, and applies the
native send checks before dispatch. A received result is retained before its
outcome is acknowledged. A failure in those stages cannot become permission to
repeat an uncertain effect.

Clock and ownership checks retain the original cutoffs. A lost source, expired
authority, uncertain continuity or failed acknowledgement may permanently fence
that invocation, including after a transient connection problem. Restoring the
connection does not renew the old authority. Preserve the original records and
inspect the external state; do not delete claims or blindly replay an operation
to make the worker proceed. A new invocation needs its own current admission.

Prepared results use private staging files followed by atomic publication that
refuses to replace an existing destination. A process interruption can leave
staging or claim files. They are diagnostic evidence, not instructions to resume.

## Debug installation channel

The selected native launcher is enabled only in debuggable installations.
Its `requestSha256` intent extra is a one-use capability for a matching request
already present in private app storage. Treat that hash as private: disclosure
can let another caller consume or trigger the pending request. The hash alone
cannot create an arbitrary request or bypass the installed association checks.

The launcher has one worker and at most eight queued requests. Overflow is
rejected before claiming a request. Missing, malformed, non-private or
hash-mismatched request files do not create claim or result files. Once a valid
request is claimed, a failure does not make it replayable. Ordinary release
installations refuse this debug channel.

These source and fixture checks do not establish device, deployment or model
qualification. The installation and exact build used for any observation must
be recorded separately.
