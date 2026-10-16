package com.trollm.app

import org.junit.Assert.*
import org.junit.Test

class RunIdentityTest {
    private val a = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    private val b = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"

    @Test fun restartDiscoversDiskRunWhenPreferenceWriteWasInterrupted() {
        assertEquals(listOf(a, b), RunIdentity.discover(listOf(a), listOf(a, b, "cache", "../escape")))
        assertEquals(listOf(b), RunIdentity.discover(emptyList(), listOf(b)))
    }

    @Test fun notificationAndStoredRoutesRequireCanonicalUuid() {
        assertTrue(RunIdentity.valid(a))
        assertFalse(RunIdentity.valid(null))
        assertFalse(RunIdentity.valid("1-1-1-1-1"))
        assertFalse(RunIdentity.valid("../../outside"))
        assertFalse(RunIdentity.valid(a.uppercase()))
        assertFalse(RunIdentity.valid(" $a"))
        assertFalse(RunIdentity.valid("${a}0"))
    }

    @Test fun eitherCompletionOrderRetainsTheOtherOwner() {
        for ((first, last) in listOf(a to b, b to a)) {
            val owners = ActiveRuns()
            owners.attach(a)
            owners.attach(b)
            assertFalse(owners.complete(first))
            assertEquals(last, owners.first())
            assertTrue(owners.complete(last))
        }
    }

    @Test fun duplicateAttachmentAndLateCompletionDoNotReleaseAnotherRun() {
        val owners = ActiveRuns()
        assertTrue(owners.attach(a))
        assertFalse(owners.attach(a))
        owners.attach(b)
        assertFalse(owners.complete(a))
        assertFalse(owners.complete(a))
        assertEquals(b, owners.first())
        assertTrue(owners.complete(b))
    }

    @Test fun unknownCompletionPreservesAllOwners() {
        val owners = ActiveRuns()
        owners.attach(a)
        assertFalse(owners.complete(b))
        assertEquals(a, owners.first())
        assertFalse(owners.isEmpty())
        assertTrue(owners.complete(a))
        assertNull(owners.first())
        assertTrue(owners.isEmpty())
    }

    @Test fun invalidAttachmentDoesNotAcquireOwnership() {
        val owners = ActiveRuns()
        assertThrows(IllegalArgumentException::class.java) { owners.attach("../escape") }
        assertTrue(owners.isEmpty())
    }
}
