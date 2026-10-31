package com.trollm.app

import org.junit.Assert.*
import org.junit.Test

class RunWakeLeaseTest {
    @Test fun renewalRequiresAnAcquiredLeaseAndFiveMinutes() {
        val lease = RunWakeLease()
        assertFalse(lease.shouldRenew(600_000L, true))
        lease.acquired(50L)
        assertFalse(lease.shouldRenew(300_049L, true))
        assertTrue(lease.shouldRenew(300_050L, true))
        lease.acquired(300_050L)
        assertFalse(lease.shouldRenew(300_051L, true))
        assertFalse(lease.shouldRenew(600_049L, true))
        assertTrue(lease.shouldRenew(600_050L, true))
    }

    @Test fun freshLiveObservationsSupportMoreThanTwoHoursWithoutAContinuityGap() {
        val lease = RunWakeLease()
        lease.acquired(0L)
        var expiresAt = RunWakeLease.TIMEOUT_MS
        var renewals = 0
        for (now in 1000L..(3 * 60 * 60 * 1000L) step 1000L) {
            assertTrue("Lease expired at $now", now < expiresAt)
            if (lease.shouldRenew(now, true)) {
                lease.acquired(now)
                expiresAt = now + RunWakeLease.TIMEOUT_MS
                renewals++
            }
        }
        assertEquals(36, renewals)
    }

    @Test fun failedOrNonLiveObservationsNeverExtendEvenAnExpiredLease() {
        val lease = RunWakeLease()
        lease.acquired(0L)
        for (now in listOf(300_000L, 600_000L, 7_200_000L)) {
            assertFalse(lease.shouldRenew(now, null))
            assertFalse(lease.shouldRenew(now, false))
        }
        // Only fresh live evidence permits reacquisition after expiry.
        assertTrue(lease.shouldRenew(7_201_000L, true))
    }

    @Test fun releaseResetPreventsLateRenewalsAndAllowsAnExplicitRestart() {
        val lease = RunWakeLease()
        lease.acquired(0L)
        lease.reset()
        assertFalse(lease.shouldRenew(600_000L, true))
        lease.reset()
        lease.acquired(700_000L)
        assertFalse(lease.shouldRenew(999_999L, true))
        assertTrue(lease.shouldRenew(1_000_000L, true))
    }
}
