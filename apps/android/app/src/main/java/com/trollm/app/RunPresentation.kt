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
)

fun presentRun(phase: RunPhaseApi, workerActive: Boolean, activity: RunActivityApi?, requesting: Boolean = false): RunPresentation {
    val running = phase is RunPhaseApi.Running
    val paused = phase is RunPhaseApi.Paused
    val interrupted = !workerActive && (running || paused)
    val text = when {
        interrupted -> "Interrupted — tap Resume to restore saved progress. Models load when needed."
        phase is RunPhaseApi.Succeeded -> "Succeeded"
        phase is RunPhaseApi.Failed -> "Failed: ${phase.reason}"
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
    )
}
