package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import uniffi.trollm_uniffi.RunActivityApi
import uniffi.trollm_uniffi.RunPhaseApi

class RunPresentationTest {
    @Test fun passiveReportEligibilityFollowsActualWorkerForEveryPhase() {
        val phases = listOf(RunPhaseApi.Running, RunPhaseApi.Paused("saved"),
            RunPhaseApi.Succeeded, RunPhaseApi.Failed("cancelled"), RunPhaseApi.Failed("worker failure"))
        for (phase in phases) {
            assertFalse(presentRun(phase, true, null).canRetainSourceReport)
            assertTrue(presentRun(phase, false, null).canRetainSourceReport)
        }
    }

    @Test fun cancelledTerminalRunIsNeverPresentedAsFailure() {
        val view = presentRun(RunPhaseApi.Failed("cancelled"), false, null)
        assertEquals("Cancelled", view.text)
        assertNull(view.details)
        assertFalse(view.canResume)
        assertFalse(view.keepPolling)
    }

    @Test fun knownFailureClassesHaveFriendlySummariesWithoutSuggestingBlindRetries() {
        for (reason in listOf("model-error", "mcp-unreachable", "mcp-error", "tool-call-parse",
            "verification-failed", "artifact-io", "budget-exhausted")) {
            val view = presentRun(RunPhaseApi.Failed(reason), false, RunActivityApi.Generating("coder"))
            assertTrue(view.text.startsWith("Failed:"))
            assertFalse(view.text.contains(reason))
            assertFalse(view.text.contains("retry", ignoreCase = true))
            assertEquals(reason, view.details)
            assertFalse(view.canPause)
            assertFalse(view.canResume)
            assertFalse(view.canCancel)
            assertFalse(view.keepPolling)
            assertFalse(view.observeInBackground)
        }
    }

    @Test fun authoredAndWorkerFailureReasonsArePreservedWithoutSubstringClassification() {
        for (reason in listOf("needs operator approval", "model-error: authored detail", "Cancelled",
            "run worker error: model-error: missing /private/weights", "", "cancelled but journal failed")) {
            val view = presentRun(RunPhaseApi.Failed(reason), false, null)
            assertEquals("Failed. Review details and technical events before taking further action.", view.text)
            assertEquals(reason, view.details)
            assertFalse(view.canResume)
        }
        val paused = presentRun(RunPhaseApi.Paused("Check the document before resuming"), true, null)
        assertEquals("Paused: Check the document before resuming", paused.text)
        assertNull(paused.details)
    }

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
    @Test fun visualToneAndLabelFollowTypedPhaseDespiteMisleadingProse() {
        val cases = listOf(
            Triple(RunPhaseApi.Succeeded, "Completed", StatusTone.SUCCESS),
            Triple(RunPhaseApi.Failed("cancelled"), "Cancelled", StatusTone.NEUTRAL),
            Triple(RunPhaseApi.Failed("Completed successfully"), "Failed", StatusTone.ERROR),
            Triple(RunPhaseApi.Paused("Failed! Completed!"), "Paused", StatusTone.WARNING),
            Triple(RunPhaseApi.Running, "In progress", StatusTone.INFO),
        )
        for ((phase, label, tone) in cases) {
            val view = presentRun(phase, true, RunActivityApi.Generating("Failed"))
            assertEquals(label, view.label)
            assertEquals(tone, view.tone)
        }
        val interrupted = presentRun(RunPhaseApi.Running, false, RunActivityApi.Starting)
        assertEquals("Interrupted", interrupted.label)
        assertEquals(StatusTone.WARNING, interrupted.tone)
        assertTrue(interrupted.canResume)
        assertFalse(interrupted.canPause)
        assertFalse(interrupted.canCancel)
    }

    @Test fun activityVisibleOnlyForAnActiveRunningWorker() {
        val verifying = presentRun(RunPhaseApi.Running, true, RunActivityApi.VerifyingModel("coder"))
        val loading = presentRun(RunPhaseApi.Running, true, RunActivityApi.LoadingModel("reasoner"))
        val generating = presentRun(RunPhaseApi.Running, true, RunActivityApi.Generating("coder"))
        assertEquals("Verifying model file", verifying.activityLabel)
        assertEquals("Loading model on this phone", loading.activityLabel)
        assertEquals("Generating on this phone", generating.activityLabel)
        assertEquals("reasoner", loading.activityDetail)
        assertNull(presentRun(RunPhaseApi.Running, false, RunActivityApi.Starting).activityLabel)
        assertNull(presentRun(RunPhaseApi.Succeeded, true, RunActivityApi.Starting).activityLabel)
        assertTrue(presentRun(RunPhaseApi.Succeeded, true, null).keepPolling)
    }

    @Test fun runCommandsWaitForObservedAcknowledgment() {
        val running = RunPhaseApi.Running
        val paused = RunPhaseApi.Paused("saved")
        val cancelled = RunPhaseApi.Failed("cancelled")
        assertFalse(RunCommand.PAUSE.acknowledged(running, true, 7uL, 8uL))
        assertTrue(RunCommand.PAUSE.acknowledged(paused, true, 7uL, 8uL))
        assertFalse(RunCommand.CANCEL.acknowledged(cancelled, true, 7uL, 8uL))
        assertTrue(RunCommand.CANCEL.acknowledged(cancelled, false, 7uL, 8uL))
        assertFalse(RunCommand.RESUME.acknowledged(paused, true, 7uL, 7uL))
        assertTrue(RunCommand.RESUME.acknowledged(running, true, 7uL, 7uL))
        assertTrue(RunCommand.RESUME.acknowledged(paused, true, 7uL, 8uL))
    }

    @Test fun restoredResumeRequestClearsWhenTheWorkerWasLost() {
        for (phase in listOf(RunPhaseApi.Running, RunPhaseApi.Paused("saved"))) {
            assertTrue(RunCommand.RESUME.acknowledged(phase, false, 7uL, 7uL))
            val restored = presentRun(phase, false, null)
            assertEquals("Interrupted", restored.label)
            assertTrue(restored.canResume)
        }
    }

    @Test fun failureRecoveryUsesVerifiedStarterIdentity() {
        assertNull(runFailureRecovery(BundledStarter.TEXT_SUMMARY))
        assertNull(runFailureRecovery(BundledStarter.CHECK_TOTAL))
        assertTrue(runFailureRecovery(BundledStarter.BROWSER_SUMMARY)!!.contains("opened a tab"))
        assertTrue(runFailureRecovery(null)!!.startsWith("If this flow can change"))
    }
}
