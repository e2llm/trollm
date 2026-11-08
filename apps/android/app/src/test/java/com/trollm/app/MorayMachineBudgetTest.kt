package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class MorayMachineBudgetTest {
    @Test fun unconfirmedWorkerRetirementPermanentlyFencesAnotherAcquisition() {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val stopped = CountDownLatch(1)
        val budget = MorayMachineBudget(500)
        try {
            try {
                budget.run {
                    entered.countDown()
                    try {
                        while (release.count > 0) {
                            try { release.await() } catch (_: InterruptedException) { /* declared noncooperative work */ }
                        }
                        "late value"
                    } finally { stopped.countDown() }
                }
                fail("unconfirmed worker was accepted")
            } catch (failure: MorayFailure) {
                assertEquals("SOURCE_MACHINE_TIMEOUT_STOP_UNCONFIRMED", failure.code)
            }
            assertEquals(0L, entered.count)
            assertEquals(1L, stopped.count)
            release.countDown()
            assertTrue(stopped.await(3, TimeUnit.SECONDS))
            try { budget.run { fail("fenced work started") }; fail("retirement fence reset") }
            catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_TEARDOWN_UNCONFIRMED", failure.code) }
        } finally { release.countDown(); assertTrue(stopped.await(3, TimeUnit.SECONDS)) }
    }
}
