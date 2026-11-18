package com.trollm.app

import android.content.Context
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.trollm_uniffi.TrollmRuntime

internal data class RunStartRequest(
    val flowSha: String,
    val sourceInstallation: String?,
    val sourceAssociation: String?,
    val submittedInput: String,
    val originalInput: String,
    val configuration: String?,
    val declaredSource: Boolean,
)

internal data class RunStartResult(val runId: String, val warning: UiError?)

internal sealed interface RunStartPhase {
    data object Pending : RunStartPhase
    data class Unconfirmed(val diagnostic: String? = null) : RunStartPhase
    data class Started(val result: RunStartResult, val acknowledged: Boolean = false) : RunStartPhase
    data class NotDispatched(val error: UiError) : RunStartPhase
}

internal data class RunStartRecord(val screenKey: String, val phase: RunStartPhase)

private const val START_RECEIPT = "run-start-receipt"

internal class RunStartViewModel(savedState: SavedStateHandle) : ViewModel() {
    private val handoff = RunStartHandoff(
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate),
        restored = savedState.get<ArrayList<String>>(START_RECEIPT),
        persist = { receipt ->
            if (receipt == null) savedState.remove<ArrayList<String>>(START_RECEIPT)
            else savedState[START_RECEIPT] = ArrayList(receipt)
        },
    )
    val record: StateFlow<RunStartRecord?> = handoff.record

    fun start(screenKey: String, context: Context, runtime: TrollmRuntime, request: RunStartRequest): Boolean {
        val application = context.applicationContext
        return handoff.submit(screenKey) { startAndRecord(application, runtime, request) }
    }

    fun acknowledge(screenKey: String, runId: String) = handoff.acknowledge(screenKey, runId)

    override fun onCleared() {
        handoff.close()
    }
}

/** Called on the owner's dispatcher. An accepted operation finishes even after the owner closes. */
internal class RunStartHandoff(
    private val scope: CoroutineScope,
    restored: List<String>?,
    private val persist: (List<String>?) -> Unit,
) {
    private val latest = MutableStateFlow(restoreRunStart(restored))
    val record = latest.asStateFlow()
    private var closed = false

    fun submit(screenKey: String, operation: suspend () -> RunStartResult): Boolean {
        val current = latest.value
        val phase = current?.phase
        if (closed || phase == RunStartPhase.Pending ||
            (phase is RunStartPhase.Started && !phase.acknowledged) ||
            (phase is RunStartPhase.Unconfirmed && current?.screenKey == screenKey)) return false

        val initialError = persistReceipt(listOf(screenKey), "Could not save the start request on this device.")
        if (initialError != null) {
            latest.value = RunStartRecord(screenKey, RunStartPhase.NotDispatched(initialError))
            return false
        }
        latest.value = RunStartRecord(screenKey, RunStartPhase.Pending)
        scope.launch {
            try {
                catchingOperation { operation() }
                    .onSuccess { result ->
                        val receiptError = persistReceipt(
                            listOf(screenKey, result.runId),
                            "The run started, but its navigation receipt could not be saved.",
                        )
                        latest.value = RunStartRecord(screenKey, RunStartPhase.Started(result.copy(
                            warning = if (receiptError == null) result.warning else combineUiErrors(result.warning, receiptError),
                        )))
                    }
                    .onFailure { failure ->
                        val rejectedPhase = if (failure is StartPreflightRejected) {
                            val rejected = operationError(UiOperation.START_RUN, failure.original)
                            val receiptError = persistReceipt(null, "Could not clear the start request's receipt.")
                            RunStartPhase.NotDispatched(
                                if (receiptError == null) rejected else checkNotNull(combineUiErrors(rejected, receiptError)),
                            )
                        } else {
                            // The bridge can fail after allocating the run. Preserve the pending receipt.
                            RunStartPhase.Unconfirmed(operationError(UiOperation.START_RUN, failure).details)
                        }
                        latest.value = RunStartRecord(screenKey, rejectedPhase)
                    }
            } catch (cancelled: CancellationException) {
                latest.value = RunStartRecord(screenKey, RunStartPhase.Unconfirmed())
                throw cancelled
            } finally {
                if (closed) scope.cancel()
            }
        }
        return true
    }

    fun acknowledge(screenKey: String, runId: String) {
        val current = latest.value ?: return
        val phase = current.phase as? RunStartPhase.Started ?: return
        if (current.screenKey != screenKey || phase.result.runId != runId || phase.acknowledged) return
        val receiptError = persistReceipt(null, "Could not clear the run's navigation receipt.")
        val result = phase.result
        val warning = if (receiptError == null) result.warning else combineUiErrors(result.warning, receiptError)
        latest.value = current.copy(phase = RunStartPhase.Started(result.copy(warning = warning), acknowledged = true))
    }

    private fun persistReceipt(receipt: List<String>?, summary: String): UiError? =
        runCatching { persist(receipt) }.exceptionOrNull()?.let { failure ->
            UiError(summary, failure.message?.takeIf(String::isNotBlank) ?: failure.javaClass.simpleName)
        }

    fun close() {
        closed = true
        if (latest.value?.phase != RunStartPhase.Pending) scope.cancel()
    }
}

private fun restoreRunStart(receipt: List<String>?): RunStartRecord? {
    val entries = receipt ?: return null
    val screenKey = entries.firstOrNull()?.takeIf(String::isNotBlank) ?: return null
    val id = entries.getOrNull(1)?.takeIf(RunIdentity::valid)
    return RunStartRecord(screenKey, if (id == null) RunStartPhase.Unconfirmed()
        else RunStartPhase.Started(RunStartResult(id, null)))
}

private class StartPreflightRejected(val original: Throwable) : Exception(original)

/** Scope this marker to checks that run before calling the native Start bridge. */
internal suspend fun <T> checkRunStartPreflight(block: suspend () -> T): T =
    catchingOperation { block() }.getOrElse { throw StartPreflightRejected(it) }

private suspend fun startAndRecord(
    context: Context, runtime: TrollmRuntime, request: RunStartRequest,
): RunStartResult = withContext(Dispatchers.IO) {
    val installation = request.sourceInstallation
    val association = checkRunStartPreflight {
        check(!runtime.hasActiveRun()) { "Finish or cancel the active run before starting another." }
        if (installation != null) runInterruptible {
            MoraySource.authorizeFreshStart(context, JSONObject(checkNotNull(request.sourceAssociation)))
        }
        if (request.declaredSource) {
            SourceInputAdmission.attach(
                checkNotNull(request.sourceAssociation), request.originalInput, request.submittedInput,
            ) { raw, apk -> runtime.admitSourceInput(checkNotNull(installation), raw, apk) }
        } else request.sourceAssociation
    }
    val executionInput = if (request.declaredSource) request.originalInput else request.submittedInput
    val configuration = request.configuration
    val id = if (installation == null) {
        if (configuration == null) runtime.startRun(request.flowSha, request.submittedInput)
        else runtime.startRunConfigured(request.flowSha, request.submittedInput, configuration)
    } else {
        if (configuration == null) runtime.startSourceRun(request.flowSha, installation, executionInput, checkNotNull(association))
        else runtime.startSourceRunConfigured(request.flowSha, installation, executionInput, configuration, checkNotNull(association))
    }
    val storageError = catchingOperation { RunHistory(context).record(id) }.exceptionOrNull()
    val serviceError = catchingOperation { RunService.start(context, id) }.exceptionOrNull()
    RunStartResult(id, combineUiErrors(
        storageError?.let { operationError(UiOperation.SAVE_HISTORY, it) },
        serviceError?.let { operationError(UiOperation.BACKGROUND_PROGRESS, it) },
    ))
}
