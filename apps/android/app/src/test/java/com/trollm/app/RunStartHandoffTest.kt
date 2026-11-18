package com.trollm.app

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RunStartHandoffTest {
    private val runId = "ab59a1ce-564a-4fe0-b15b-dab75e40f848"
    private val warning = UiError("Could not enable background progress.", "service failure")

    @Test fun recreatedObserverReceivesTheSameStartAndWarning() = runBlocking {
        val finish = CompletableDeferred<RunStartResult>()
        val scope = ownerScope()
        val handoff = RunStartHandoff(scope, null) {}
        var dispatches = 0
        assertTrue(handoff.submit("flow-1") {
            dispatches++
            finish.await()
        })
        // The first observer detaches while native Start is still in progress.
        assertEquals(RunStartPhase.Pending, handoff.record.first()?.phase)
        finish.complete(RunStartResult(runId, warning))
        val recreated = handoff.record.first { it?.phase is RunStartPhase.Started }
        assertEquals(RunStartRecord("flow-1", RunStartPhase.Started(RunStartResult(runId, warning))), recreated)
        assertEquals(1, dispatches)
        handoff.close()
    }

    @Test fun aGenuineUuidSurvivesUntilTheMatchingNavigationAcknowledgesIt() {
        val scope = ownerScope()
        var receipt: List<String>? = null
        val handoff = RunStartHandoff(scope, null) { receipt = it }
        assertTrue(handoff.submit("flow-1") { RunStartResult(runId, warning) })
        assertEquals(listOf("flow-1", runId), receipt)
        handoff.acknowledge("other-flow", runId)
        assertEquals(listOf("flow-1", runId), receipt)
        val restored = RunStartHandoff(ownerScope(), receipt) {}
        assertEquals(RunStartRecord("flow-1", RunStartPhase.Started(RunStartResult(runId, null))), restored.record.value)
        restored.close()
        handoff.acknowledge("flow-1", runId)
        assertNull(receipt)
        assertEquals(RunStartPhase.Started(RunStartResult(runId, warning), acknowledged = true), handoff.record.value?.phase)
        handoff.close()
    }

    @Test fun restoredPendingIsUnconfirmedAndCannotResubmit() {
        val handoff = RunStartHandoff(ownerScope(), listOf("flow-1")) {}
        assertEquals(RunStartRecord("flow-1", RunStartPhase.Unconfirmed()), handoff.record.value)
        assertFalse(handoff.submit("flow-1") { error("Unconfirmed Start was dispatched again") })
        handoff.close()
    }

    @Test fun preflightRejectionLeavesTheSameDraftAvailableForAnExplicitRetry() {
        var receipt: List<String>? = null
        var active = true
        var dispatches = 0
        val handoff = RunStartHandoff(ownerScope(), null) { receipt = it }
        val attempt: suspend () -> RunStartResult = {
            checkRunStartPreflight { check(!active) { "active run diagnostic" } }
            dispatches++
            RunStartResult(runId, null)
        }

        assertTrue(handoff.submit("flow-1", attempt))
        assertEquals(0, dispatches)
        assertNull(receipt)
        val rejection = handoff.record.value?.phase as RunStartPhase.NotDispatched
        assertEquals("active run diagnostic", rejection.error.details)

        active = false
        assertTrue(handoff.submit("flow-1", attempt))
        assertEquals(1, dispatches)
        assertEquals(listOf("flow-1", runId), receipt)
        handoff.close()
    }

    @Test fun operationFailureRemainsUnconfirmedAcrossRestoration() {
        var receipt: List<String>? = null
        val handoff = RunStartHandoff(ownerScope(), null) { receipt = it }
        assertTrue(handoff.submit("flow-1") { error("native startup diagnostic") })
        assertEquals(listOf("flow-1"), receipt)
        assertEquals(RunStartPhase.Unconfirmed("native startup diagnostic"), handoff.record.value?.phase)
        assertFalse(handoff.submit("flow-1") { error("Failed Start was dispatched again") })
        handoff.close()

        val restored = RunStartHandoff(ownerScope(), receipt) {}
        assertEquals(RunStartPhase.Unconfirmed(), restored.record.value?.phase)
        assertFalse(restored.submit("flow-1") { error("Restored Start was dispatched again") })
        restored.close()
    }

    @Test fun successfulUuidRemainsAvailableWhenTheSavedReceiptCannotBeUpdated() {
        var receipt: List<String>? = null
        val handoff = RunStartHandoff(ownerScope(), null) {
            if (it?.size == 2) error("could not update receipt")
            receipt = it
        }
        assertTrue(handoff.submit("flow-1") { RunStartResult(runId, warning) })
        val started = handoff.record.value?.phase as RunStartPhase.Started
        assertEquals(runId, started.result.runId)
        val details = requireNotNull(started.result.warning?.details)
        assertTrue(details.contains("could not update receipt"))
        assertTrue(details.contains("service failure"))
        assertEquals(listOf("flow-1"), receipt)
        handoff.acknowledge("flow-1", runId)
        assertNull(receipt)
        handoff.close()
    }

    @Test fun failureToSaveThePendingReceiptDoesNotDispatch() {
        var dispatched = false
        val handoff = RunStartHandoff(ownerScope(), null) { error("storage diagnostic") }
        assertFalse(handoff.submit("flow-1") {
            dispatched = true
            RunStartResult(runId, null)
        })
        assertFalse(dispatched)
        val failed = handoff.record.value?.phase as RunStartPhase.NotDispatched
        assertEquals("storage diagnostic", failed.error.details)
        handoff.close()
    }

    @Test fun duplicatePendingSubmissionsCannotDispatch() = runBlocking {
        val finish = CompletableDeferred<RunStartResult>()
        val handoff = RunStartHandoff(ownerScope(), null) {}
        assertTrue(handoff.submit("flow-1") { finish.await() })
        assertFalse(handoff.submit("flow-1") { error("Duplicate Start was dispatched") })
        assertFalse(handoff.submit("flow-2") { error("Concurrent Start was dispatched") })
        finish.complete(RunStartResult(runId, null))
        handoff.record.first { it?.phase is RunStartPhase.Started }
        handoff.close()
    }

    @Test fun finishingTheActivityAllowsTheAcceptedHandoffToRecordItsUuid() = runBlocking {
        val finish = CompletableDeferred<RunStartResult>()
        val scope = ownerScope()
        var receipt: List<String>? = null
        val handoff = RunStartHandoff(scope, null) { receipt = it }
        assertTrue(handoff.submit("flow-1") { finish.await() })
        handoff.close()
        assertTrue(scope.coroutineContext[Job]!!.isActive)
        assertFalse(handoff.submit("flow-2") { error("Closed owner accepted a new Start") })
        finish.complete(RunStartResult(runId, null))
        handoff.record.first { it?.phase is RunStartPhase.Started }
        scope.coroutineContext[Job]!!.join()
        assertEquals(listOf("flow-1", runId), receipt)
        assertFalse(scope.coroutineContext[Job]!!.isActive)
    }

    private fun ownerScope() = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
}
