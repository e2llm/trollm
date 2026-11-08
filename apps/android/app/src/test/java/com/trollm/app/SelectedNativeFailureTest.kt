package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import uniffi.trollm_uniffi.TrollmException

class SelectedNativeFailureTest {
    @Test fun onlyTypedClosedCauseAndOriginalAllocatedIdCrossThePrivateResult() {
        val id = "00000000-0000-4000-8000-000000000001"
        val result = JSONObject()
        SelectedNativeLauncher.retainClosedStartFailure(result,
            TrollmException.SelectedStart("SELECTED_BIND_STATIC_KEY_HASH", id))
        assertEquals("SELECTED_BIND_STATIC_KEY_HASH", result.getString("nativeCause"))
        assertEquals(id, result.getString("nativeRunId"))
        assertFalse(result.has("binding"))
        assertFalse(result.has("workerActive"))
        assertFalse(result.has("workerInactiveAcknowledged"))
    }

    @Test fun arbitraryExceptionTextAndInvalidTypedFieldsAreNotCopied() {
        for (failure in listOf(Exception("private-secret-source"),
            TrollmException.SelectedStart("private-secret-source", "not-a-native-id"))) {
            val result = JSONObject()
            SelectedNativeLauncher.retainClosedStartFailure(result, failure)
            assertEquals(0, result.length())
        }
    }
}
