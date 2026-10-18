package com.trollm.app

import java.util.concurrent.CancellationException

/** Presentation only: technical diagnostics remain unchanged, not redacted. */
data class UiError(val summary: String, val details: String? = null)

enum class UiOperation(val summary: String) {
    LOAD_FLOWS("Could not load installed flows."),
    INSTALL_STARTERS("Could not prepare the supplied flows."),
    LOAD_SETUP("Could not load this flow's setup."),
    READ_HISTORY("Could not read some run history."),
    PREPARE_INPUT("Check the input fields before starting."),
    START_RUN("Could not start the run. Check model setup and connections."),
    SAVE_HISTORY("The run started, but its history entry could not be saved."),
    BACKGROUND_PROGRESS("Background progress could not be enabled. Open the run to check its status."),
    OBSERVE_RUN("Could not update run progress. The run may still be active."),
    PAUSE_RUN("Could not pause the run. Check its current status."),
    RESUME_RUN("Could not resume the run. Check its current status before taking further action."),
    CANCEL_RUN("Could not cancel the run. It may still be active."),
}

fun operationError(operation: UiOperation, failure: Throwable): UiError {
    if (failure is CancellationException) throw failure
    return UiError(operation.summary, failure.message?.takeIf { it.isNotBlank() } ?: failure.javaClass.simpleName)
}

fun combineUiErrors(vararg errors: UiError?): UiError? {
    val present = errors.filterNotNull()
    if (present.isEmpty()) return null
    return UiError(present.map { it.summary }.distinct().joinToString("\n"),
        present.joinToString("\n\n") { error -> error.summary + (error.details?.let { "\n$it" } ?: "") })
}

/** Cancellation is lifecycle control, never a user-visible operation failure. */
inline fun <T> catchingOperation(block: () -> T): Result<T> = try {
    Result.success(block())
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (failure: Throwable) {
    Result.failure(failure)
}
