package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import uniffi.trollm_uniffi.RunActivityApi
import uniffi.trollm_uniffi.RunPhaseApi

class RunPresentationTest {
    @Test fun liveOperationsDescribeTheActualWork() {
        val operations = listOf(
            RunActivityApi.Starting to "Starting…",
            RunActivityApi.VerifyingModel("coder") to "Verifying coder model…",
            RunActivityApi.LoadingModel("reasoner") to "Loading reasoner model…",
            RunActivityApi.Generating("coder") to "Generating with coder…",
        )
        for ((activity, expected) in operations) {
            val view = presentRun(RunPhaseApi.Running, true, activity)
            assertEquals(expected, view.text)
            assertTrue(view.canPause)
            assertTrue(view.canCancel)
            assertFalse(view.canResume)
            assertTrue(view.observeInBackground)
            assertTrue(view.keepPolling)
        }
    }

    @Test fun diskSnapshotsNeverPretendAWorkerIsStillRunning() {
        for (phase in listOf(RunPhaseApi.Running, RunPhaseApi.Paused("saved"))) {
            val view = presentRun(phase, false, RunActivityApi.Generating("coder"))
            assertTrue(view.text.startsWith("Interrupted"))
            assertTrue(view.canResume)
            assertFalse(view.canPause)
            assertFalse(view.canCancel)
            assertFalse(view.observeInBackground)
            assertFalse(view.keepPolling)
        }
    }

    @Test fun pausedWorkerReleasesBackgroundObservationButCanResumeOrCancel() {
        val view = presentRun(RunPhaseApi.Paused("needs input"), true, null)
        assertEquals("Paused: needs input", view.text)
        assertTrue(view.canResume)
        assertTrue(view.canCancel)
        assertFalse(view.canPause)
        assertFalse(view.observeInBackground)
        assertTrue(view.keepPolling)
    }

    @Test fun pendingControlRequestsDisableEveryControl() {
        for (phase in listOf(RunPhaseApi.Running, RunPhaseApi.Paused("held"))) {
            for (worker in listOf(true, false)) {
                val view = presentRun(phase, worker, null, requesting = true)
                assertFalse(view.canPause)
                assertFalse(view.canResume)
                assertFalse(view.canCancel)
            }
        }
    }

    @Test fun terminalStatusWinsOverStaleActivityAndStopsObservation() {
        for (phase in listOf(RunPhaseApi.Succeeded, RunPhaseApi.Failed("Cannot restore startup; start a new run."))) {
            val view = presentRun(phase, false, RunActivityApi.LoadingModel("coder"))
            assertFalse(view.text.contains("Loading"))
            assertFalse(view.canPause)
            assertFalse(view.canResume)
            assertFalse(view.canCancel)
            assertFalse(view.observeInBackground)
            assertFalse(view.keepPolling)
        }
    }
}
