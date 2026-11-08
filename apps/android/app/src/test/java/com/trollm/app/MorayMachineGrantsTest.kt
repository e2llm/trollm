package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class MorayMachineGrantsTest {
    private val first = "a".repeat(64)
    private val second = "b".repeat(64)
    private fun value(profile: String) = JSONObject().put("profile", profile).put("grantId", "declared-lineage")
    private fun reject(work: () -> Unit) {
        try { work(); fail("accepted stale provisioning") }
        catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_CREDENTIAL_CHANGED", failure.code) }
    }

    @Test fun principalReplacementIsExclusiveAndOriginalRevisionCannotWriteLate() {
        var durable = MorayMachineState(null, null)
        val grants = MorayMachineGrants({ durable }, { durable = it }, Any())
        grants.replace(first, null, value(first))
        val old = grants.state(first)
        grants.replace(second, old.revision, value(second))
        assertNull(grants.state(first).value)
        assertEquals(second, grants.state(second).value!!.sourceString("profile"))
        try { grants.remove(first); fail("removal claimed for the wrong profile") }
        catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_PROFILE_MISMATCH", failure.code) }
        assertNotNull(grants.state(second).value)
        reject { grants.replace(first, old.revision, value(first)) }
        assertNull(grants.state(first).value)
    }

    @Test fun absentRemovalAndWriteFailureNeverEnablePendingImport() {
        var durable = MorayMachineState(null, null)
        val grants = MorayMachineGrants({ durable }, { durable = it }, Any())
        val pending = grants.state(first)
        grants.remove(first)
        assertNotNull(grants.state(first).revision)
        reject { grants.replace(first, pending.revision, value(first)) }
        assertNull(grants.state(first).value)
        val failing = MorayMachineGrants({ durable }, { throw IllegalStateException("declared storage failure") }, Any())
        try { failing.replace(first, durable.revision, value(first)); fail("write failure hidden") }
        catch (_: IllegalStateException) {}
        assertNull(grants.state(first).value)
    }

    @Test fun selectedRemovalFencesCurrentGenerationWithoutChangingRetainedGrantBytes() {
        var durable = MorayMachineState(null, null)
        val grants = MorayMachineGrants({ durable }, { durable = it }, Any())
        val original = value(first)
        val bytes = original.toString()
        grants.replace(first, null, original)
        val before = grants.state(first)
        grants.remove(first)
        assertNull(grants.state(first).value)
        assertNotEquals(before.revision, grants.state(first).revision)
        assertEquals(bytes, original.toString())
    }
}
