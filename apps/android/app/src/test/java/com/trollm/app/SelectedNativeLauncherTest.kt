package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class SelectedNativeLauncherTest {
    private val revision = "d".repeat(40)
    private fun request(operation: String): JSONObject = JSONObject()
        .put("schema", "trollm.private-selected-launcher-request.v1")
        .put("operation", operation).put("apkSha256", "a".repeat(64))
        .put("installationKey", "b".repeat(64)).put("invocationKey", "declared-only")
        .put("requestId", "c".repeat(32)).put("sourceRevision", revision)
        .also {
            if (operation == "start") {
                it.put("flowSha256", "e".repeat(64)).put("installationSha256", "f".repeat(64))
                it.put("input", "{}").put("configurationJson", "{}").put("associationJson", "{}")
            } else it.put("nativeRunId", "11111111-2222-4333-8444-555555555555")
        }

    @Test fun closedStartAndSameRunOperations() {
        for (operation in listOf("start", "binding", "cancel")) {
            assertEquals(operation, SelectedNativeLauncher.validateRequest(request(operation), revision))
        }
    }

    @Test fun unknownFieldsCoercionWrongSourceAndReplayIdsRefuse() {
        for (change in listOf<(JSONObject) -> Unit>(
            { it.put("verified", true) }, { it.put("invocationKey", 1) },
            { it.put("sourceRevision", "0".repeat(40)) }, { it.put("requestId", "../escape") },
            { it.put("installationKey", "../escape") }, { it.put("apkSha256", "not-a-hash") },
            { it.remove("associationJson") }, { it.put("operation", "resume") },
        )) {
            val input = request("start"); change(input)
            assertThrows(Exception::class.java) { SelectedNativeLauncher.validateRequest(input, revision) }
        }
        val other = request("binding").put("nativeRunId", "missing")
        assertThrows(Exception::class.java) { SelectedNativeLauncher.validateRequest(other, revision) }
    }
}
