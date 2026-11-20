# Prepare models during an explicitly started run

The Android bridge prepares up to two task-referenced
model slots after constructing or restoring the executor for an explicit Start
or Resume. The coder is scheduled first when present. Preparation can overlap
the other slot's preparation and foreground model/tool work; inference remains
serial on the executor's worker. Plans referencing more than two distinct slots
retain first-use lazy loading for all slots, avoiding an extra foreground load
alongside two background loaders.
Opening the app, setup or history never starts preparation or resumes a run.

Explicit `execution: tool` tasks reference no model slot and never schedule model
preparation. A direct-only plan uses `models: {}` and needs no model files or
inference backend. Mixed flows prepare their actual model-task slots as before.
Declared-but-unused model slots retain the existing setup/configuration rules;
authors should remove unused declarations rather than expect implicit rewriting.

Each preparation verifies the configured SHA-256 before constructing the native
model. It uses the saved effective configuration on Resume and retains the loaded
weights for that executor. No weights are downloaded, copied or loaded from
temporary storage by this change. A referenced slot can be prepared even when
the chosen route never reaches it; a preparation error is retained and reported
if that slot is used, without bypassing integrity checks or retrying a load.

## Ownership, controls and timing

- At most two preparation workers belong to one run. They never generate text
  or dispatch tools. The shared llama backend already owns process-wide setup.
- The saved root plan deadline bounds run-wide preparation, including on cold
  Resume. Preparing ahead does not reset a run's clock. A child that returns
  while another slot loads must not leave its shorter deadline attached to that
  slot. Foreground generation and waiting still obey the earliest currently
  active frame deadline and the request's own remaining generation allowance.
- Hashing and preparation boundaries observe pause, cancel and the run deadline.
  Background waits do not duplicate foreground pause accounting. Foreground
  cooperative holds retain their existing Resume/Cancel boundary; an expired
  allowance prevents generation when that boundary is released. A native load
  already in progress is synchronous: shutdown joins it before declaring the
  worker inactive. Cancel is not an instantaneous interruption of native loading.
- The foreground operation owns the UI activity. A model waiting for its weights
  shows verification/loading; a different background slot cannot replace the
  foreground generation label. Background preparation is visible when the
  foreground has no activity to report.
- Native inference still creates a fresh context for each request. Preparing
  weights is not prompt/KV caching and does not remove repeated prefill costs.

First consumption of a prepared result reports `preparationAsync: true` and its
`weightVerifyMs`/`loadMs` intervals. Those intervals may overlap other work and
need not fall wholly inside that call's `ms`/`totalMs`. They must not be added
to it or summed as disjoint phases of it.
`preparationWaitMs` records the foreground wait inside the call, including a
wait that times out before preparation finishes. Later warm calls do not repeat
the loading costs. See [measurement boundaries](amazon-generalization/PERFORMANCE.md).
Preparation of a slot never consumed by the flow has no model-call event; these
fields are not a complete accounting of all background CPU or I/O work.

The foreground service uses a ten-minute, non-reference-counted wake-lock lease,
renewable every five minutes after a successful observation of active work.
Read errors cannot renew it. Pause/completion and destruction release ownership.
This allows long bounded diagnostic runs without the former one-hour cutoff or
an unbounded initial acquisition. Renewal confirms an active owner, not progress
inside a native call. It does not interrupt a stuck native load or guarantee
exemption from Android power management or process reclamation.

## Validation and remaining acceptance

Host tests use fake loaders and synchronization to check overlapping preparation,
checksum rejection, reuse, controls, wait deadlines, activity and worker cleanup.
Cold-resume regression coverage checks that a completed child's deadline does
not invalidate preparation for its parent; active-child limits still apply.
Pure JVM tests cover the wake-lock renewal decision; Android/native CI compiles
the actual bridge and service. None establishes phone memory pressure, speedup
or a complete browser workflow. Two simultaneous loads can compete for CPU and
storage bandwidth; the phone comparison must measure those effects.

The initial delivery checklist is: focused Rust/JVM/Python checks, native Android
build and security gate, then the phone owner's single bounded diagnostic with
the exact new APK and flow package. Source changes alone do not update an
installed APK. Prefer reaching and verifying the complete intended result before
optimizing call count; keep mutation, evidence and recovery checks intact.
