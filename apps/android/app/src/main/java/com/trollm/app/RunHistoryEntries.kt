package com.trollm.app

import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.RunSummaryInfo

/** Presentation derived from persisted metadata. Every discovered ID keeps a row. */
data class RunHistoryEntry(
    val id: String,
    val flowTitle: String,
    val flowId: String?,
    val startedAtMs: Long?,
    val status: String,
    val tone: StatusTone = StatusTone.NEUTRAL,
)

fun historyEntries(
    ids: List<String>,
    summaries: Map<String, RunSummaryInfo>,
    titlesBySha: Map<String, String>,
): List<RunHistoryEntry> = ids.map { id ->
    val summary = summaries[id]
    val flowId = summary?.flowId
    val title = summary?.flowSha256?.let(titlesBySha::get)
    val phase = summary?.phase
    val status = when (phase) {
        is RunPhaseApi.Running -> if (summary?.workerActive == true) "Running" else "Interrupted · Resume"
        is RunPhaseApi.Paused -> if (summary?.workerActive == true) "Paused" else "Saved pause · Resume"
        is RunPhaseApi.Succeeded -> "Completed"
        is RunPhaseApi.Failed -> if (phase.reason == "cancelled") "Cancelled" else "Failed"
        null -> "Status unavailable"
        else -> "Status unavailable"
    }
    RunHistoryEntry(
        id = id,
        flowTitle = title ?: flowId ?: "Unknown flow",
        flowId = flowId,
        startedAtMs = summary?.startedAtMs?.takeIf { it > 0uL && it <= Long.MAX_VALUE.toULong() }?.toLong(),
        status = status,
        tone = when (phase) {
            is RunPhaseApi.Running -> if (summary?.workerActive == true) StatusTone.INFO else StatusTone.WARNING
            is RunPhaseApi.Paused -> StatusTone.WARNING
            is RunPhaseApi.Succeeded -> StatusTone.SUCCESS
            is RunPhaseApi.Failed -> if (phase.reason == "cancelled") StatusTone.NEUTRAL else StatusTone.ERROR
            else -> StatusTone.NEUTRAL
        },
    )
}.sortedWith(compareByDescending<RunHistoryEntry> { it.startedAtMs ?: Long.MIN_VALUE }.thenBy { it.id })
