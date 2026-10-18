package com.trollm.app

import java.util.concurrent.CancellationException
import org.junit.Assert.*
import org.junit.Test

class UiErrorTest {
    @Test fun operationSummariesDoNotExposeTechnicalTextAndNullMessagesRemainVisible() {
        for (operation in UiOperation.values()) {
            val diagnostic = operationError(operation, IllegalStateException("private/path remote diagnostic"))
            assertEquals(operation.summary, diagnostic.summary)
            assertFalse(diagnostic.summary.contains("private/path"))
            assertEquals("private/path remote diagnostic", diagnostic.details)
            val missing = operationError(operation, IllegalStateException())
            assertEquals(operation.summary, missing.summary)
            assertEquals("IllegalStateException", missing.details)
        }
        assertNotEquals(UiOperation.START_RUN.summary, UiOperation.CANCEL_RUN.summary)
        assertNotEquals(UiOperation.LOAD_FLOWS.summary, UiOperation.OBSERVE_RUN.summary)
    }

    @Test fun secondaryFailuresRemainAvailableAlongsideTheirOperation() {
        val history = operationError(UiOperation.SAVE_HISTORY, IllegalStateException("history diagnostic"))
        val background = operationError(UiOperation.BACKGROUND_PROGRESS, IllegalStateException("service diagnostic"))
        val combined = requireNotNull(combineUiErrors(history, null, background))
        assertTrue(combined.summary.contains(history.summary))
        assertTrue(combined.summary.contains(background.summary))
        assertTrue(requireNotNull(combined.details).contains("history diagnostic"))
        assertTrue(requireNotNull(combined.details).contains("service diagnostic"))
        assertNull(combineUiErrors(null, null))
    }

    @Test fun repeatedFriendlySummariesAppearOnceWithoutDroppingPerRunDiagnostics() {
        val first = UiError(UiOperation.READ_HISTORY.summary, "Run first: unreadable snapshot")
        val second = UiError(UiOperation.READ_HISTORY.summary, "Run second: corrupt journal")
        val combined = requireNotNull(combineUiErrors(first, second))
        assertEquals(UiOperation.READ_HISTORY.summary, combined.summary)
        assertTrue(requireNotNull(combined.details).contains(requireNotNull(first.details)))
        assertTrue(requireNotNull(combined.details).contains(requireNotNull(second.details)))
    }

    @Test fun lifecycleCancellationIsRethrownByBothCaptureAndPresentation() {
        val cancellation = CancellationException("leaving screen")
        try {
            catchingOperation<Unit> { throw cancellation }
            fail("cancellation was captured")
        } catch (caught: CancellationException) { assertSame(cancellation, caught) }
        try {
            operationError(UiOperation.START_RUN, cancellation)
            fail("cancellation became an error")
        } catch (caught: CancellationException) { assertSame(cancellation, caught) }
        val ordinary = IllegalStateException("ordinary failure")
        assertSame(ordinary, catchingOperation<Unit> { throw ordinary }.exceptionOrNull())
        assertEquals(7, catchingOperation { 7 }.getOrThrow())
    }
}
