package com.trollm.app

import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.InstallResult
import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.RunSummaryInfo

/** The order in which the app introduces its verified packaged flows. */
enum class BundledStarter {
    TEXT_SUMMARY, CHECK_TOTAL, BROWSER_SUMMARY,
}

internal fun InstallResult.installedFlow() = FlowInfo(
    sha256 = flowSha256,
    id = flowId,
    version = flowVersion,
)

internal data class LibraryFlow(
    val flow: FlowInfo,
    val defaults: FlowSetup?,
    val effective: FlowSetup?,
    val starter: BundledStarter?,
) {
    val title: String get() = defaults?.title ?: flow.id
    val description: String? get() = when (starter) {
        BundledStarter.TEXT_SUMMARY -> "Turn a short passage into a summary on this phone."
        BundledStarter.CHECK_TOTAL -> "Add two amounts and compare the total with a budget."
        BundledStarter.BROWSER_SUMMARY -> "Summarize a page or ask a question about it."
        null -> defaults?.description
    }
    val connectionRequirement: String? get() = when (starter) {
        BundledStarter.TEXT_SUMMARY, BundledStarter.CHECK_TOTAL -> "No account needed"
        BundledStarter.BROWSER_SUMMARY -> "E2LLM sign-in and a connected browser required"
        null -> effective?.connections?.takeIf { it.isNotEmpty() }?.keys?.joinToString(", ", "Connections: ")
    }

    // This is the total for a phone without these files, independent of its
    // cache. The selected flow checks the actual saved files before any download.
    val modelRequirement: String? get() = effective?.let { setup ->
        val files = remainingModelDownloads(setup.models, emptySet())
        if (files.isEmpty()) {
            "No model files required"
        } else {
            val total = modelByteSize(modelDownloadTotal(files.values.map { it.sizeBytes }))
            val count = if (files.size == 1) "1 model" else "${files.size} models"
            "$count · $total on first download"
        }
    }
}

internal fun libraryFlows(
    flows: List<FlowInfo>,
    defaults: Map<String, FlowSetup?>,
    effective: Map<String, FlowSetup?>,
    starters: Map<FlowInfo, BundledStarter>,
): List<LibraryFlow> = flows.map { flow ->
    LibraryFlow(flow, defaults[flow.sha256], effective[flow.sha256], starters[flow])
}.sortedWith(compareBy<LibraryFlow> { it.starter?.ordinal ?: Int.MAX_VALUE }
    .thenBy(String.CASE_INSENSITIVE_ORDER) { it.title }
    .thenBy { it.flow.id }
    .thenByDescending { it.flow.version }
    .thenBy { it.flow.sha256 })

internal fun featuredLibraryRun(
    history: List<RunHistoryEntry>,
    summaries: Map<String, RunSummaryInfo>,
): RunHistoryEntry? {
    val resumable = history.filter { entry ->
        when (summaries[entry.id]?.phase) {
            is RunPhaseApi.Running, is RunPhaseApi.Paused -> true
            else -> false
        }
    }
    return resumable.firstOrNull { summaries[it.id]?.workerActive == true } ?: resumable.firstOrNull()
}
