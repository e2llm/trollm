package com.trollm.app

import uniffi.trollm_uniffi.RunActivityApi
import uniffi.trollm_uniffi.RunPhaseApi

/** Presentation uses worker ownership, never elapsed silence, to identify interruption. */
data class RunPresentation(
    val text: String,
    val canPause: Boolean,
    val canResume: Boolean,
    val canCancel: Boolean,
    val observeInBackground: Boolean,
    val keepPolling: Boolean,
    val canRetainSourceReport: Boolean = false,
    val details: String? = null,
    val label: String = "Run status",
    val tone: StatusTone = StatusTone.NEUTRAL,
)

fun presentRun(phase: RunPhaseApi, workerActive: Boolean, activity: RunActivityApi?, requesting: Boolean = false): RunPresentation {
    val running = phase is RunPhaseApi.Running
    val paused = phase is RunPhaseApi.Paused
    val interrupted = !workerActive && (running || paused)
    val failure = (phase as? RunPhaseApi.Failed)?.let { presentFailure(it.reason) }
    val text = when {
        interrupted -> "Interrupted — tap Resume to restore saved progress. Models load when needed."
        phase is RunPhaseApi.Succeeded -> "Completed"
        phase is RunPhaseApi.Failed -> requireNotNull(failure).summary
        paused -> "Paused: ${(phase as RunPhaseApi.Paused).reason}"
        else -> when (activity) {
            is RunActivityApi.Starting -> "Starting…"
            is RunActivityApi.VerifyingModel -> "Verifying ${activity.slot} model…"
            is RunActivityApi.LoadingModel -> "Loading ${activity.slot} model…"
            is RunActivityApi.Generating -> "Generating with ${activity.slot}…"
            null -> "Running"
        }
    }
    return RunPresentation(
        text = text,
        canPause = !requesting && workerActive && running,
        canResume = !requesting && (paused || interrupted),
        canCancel = !requesting && workerActive && (running || paused),
        observeInBackground = workerActive && running,
        keepPolling = workerActive && (running || paused),
        canRetainSourceReport = !workerActive,
        details = failure?.details,
        label = when {
            interrupted -> "Interrupted"
            phase is RunPhaseApi.Succeeded -> "Completed"
            phase is RunPhaseApi.Failed -> if (phase.reason == "cancelled") "Cancelled" else "Failed"
            paused -> "Paused"
            else -> "In progress"
        },
        tone = when {
            interrupted || paused -> StatusTone.WARNING
            phase is RunPhaseApi.Succeeded -> StatusTone.SUCCESS
            phase is RunPhaseApi.Failed -> if (phase.reason == "cancelled") StatusTone.NEUTRAL else StatusTone.ERROR
            else -> StatusTone.INFO
        },
    )
}

/** Exact classes are the runtime contract; authored reasons are never guessed. */
fun presentFailure(reason: String): UiError = when (reason) {
    "cancelled" -> UiError("Cancelled")
    "model-error" -> UiError("Failed: the model could not complete this step. Check model setup and technical events.", reason)
    "mcp-unreachable" -> UiError("Failed: a connected service could not be reached. Check the connection and network.", reason)
    "mcp-error" -> UiError("Failed: a connected service reported an error. Check technical events before further action.", reason)
    "tool-call-parse" -> UiError("Failed: the model's tool request could not be understood. Check technical events.", reason)
    "verification-failed" -> UiError("Failed: the step did not meet its completion requirements. Check technical events.", reason)
    "artifact-io" -> UiError("Failed: run data could not be safely read or saved. Check storage and details before further action.", reason)
    "budget-exhausted" -> UiError("Failed: the run reached an execution limit. Check the flow's limits and technical events.", reason)
    else -> UiError("Failed. Review details and technical events before taking further action.", reason)
}
