package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.fail
import org.junit.Test

class SelectedNativeRunTest {
    private val run = "00000000-0000-4000-8000-000000000001"
    private val key = "a".repeat(64)
    private fun binding(): JSONObject = JSONObject()
        .put("schema", "trollm.selected-worker-binding.v1").put("nativeRunId", run)
        .put("workerActive", true)
        .put("privateBindingJson", JSONObject().put("privateInstallationSha256", key)
            .put("nativeRunId", run).put("invocationKey", "original").toString())
        .put("claimJson", JSONObject().put("nativeRunId", run)
            .put("identity", JSONObject().put("invocationKey", "original")).toString())
        .put("sourceAssociationJson", JSONObject().put("nativeRunId", run).toString())

    @Test fun keepsOriginalRecordStringsAndActualBoolean() {
        val value = binding()
        val raw = value.getString("claimJson").dropLast(1) + ",\"attempt\":18446744073709551615}"
        value.put("claimJson", raw)
        SelectedNativeRun.validateBinding(value, run, key, "original")
        assertEquals(raw, value.getString("claimJson"))
        value.put("workerActive", false)
        SelectedNativeRun.validateBinding(value, run, key, "original")
    }

    @Test fun refusesWrongRunInvocationPrivateInputAndCoercedInactive() {
        val bad = listOf(
            binding().put("nativeRunId", "different"),
            binding().put("privateBindingJson", JSONObject().put("privateInstallationSha256", key)
                .put("nativeRunId", run).put("invocationKey", "different").toString()),
            binding().put("claimJson", JSONObject().put("nativeRunId", "different")
                .put("identity", JSONObject().put("invocationKey", "original")).toString()),
            binding().put("sourceAssociationJson", JSONObject().put("nativeRunId", "different").toString()),
            binding().put("workerActive", "false"),
        )
        for (value in bad) {
            try { SelectedNativeRun.validateBinding(value, run, key, "original"); fail("Expected refusal") }
            catch (_: IllegalArgumentException) { }
        }
        try { SelectedNativeRun.validateBinding(binding(), run, "b".repeat(64), "original"); fail("Expected refusal") }
        catch (_: IllegalArgumentException) { }
    }
}
