package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class SourceInputAdmissionTest {
    private fun association() = JSONObject().put("descriptor", JSONObject()
        .put("source", JSONObject().put("origin", "DECLARED")))
        .put("installation", JSONObject().put("installationSha256", "a".repeat(64)))
    private fun nativeReceipt(normalized: String) = JSONObject()
        .put("schema", "trollm.source-input-admission.v1").put("apkNormalized", normalized)
        .put("raw", JSONObject().put("sha256", "b".repeat(64)).put("sizeBytes", 73))

    @Test fun forwardsOriginalBytesAndIndependentApkNormalizationToNativeOnce() {
        val original = " {\"selectionSha256\":\"  ${"c".repeat(64)}  \"} \n"
        val fields = listOf(StructuredInputField("selectionSha256", "Selection", required = true, maxLength = 64))
        val normalized = StructuredInput.normalizeOriginal(fields, original)
        assertEquals("c".repeat(64), JSONObject(normalized).getString("selectionSha256"))
        var calls = 0
        val before = association()
        val result = SourceInputAdmission.attach(before.toString(), original, normalized) { raw, apk ->
            calls++
            assertEquals(original, raw)
            assertEquals(normalized, apk)
            nativeReceipt(apk).toString()
        }
        assertEquals(1, calls)
        assertTrue(!before.has("inputAdmission"))
        val admitted = JSONObject(result)
        assertTrue(JsonValues.same(before.getJSONObject("installation"), admitted.getJSONObject("installation")))
        assertTrue(JsonValues.same(nativeReceipt(normalized), admitted.getJSONObject("inputAdmission")))
    }

    @Test fun refusesConflictingSuppliedAdmissionInsteadOfReplacingIt() {
        val normalized = "{\"selectionSha256\":\"${"c".repeat(64)}\"}"
        val source = association().put("inputAdmission", nativeReceipt(normalized).put("raw", JSONObject().put("sha256", "wrong")))
        assertThrows(IllegalArgumentException::class.java) {
            SourceInputAdmission.attach(source.toString(), normalized, normalized) { _, apk -> nativeReceipt(apk).toString() }
        }
        val identical = association().put("inputAdmission", nativeReceipt(normalized))
        SourceInputAdmission.attach(identical.toString(), normalized, normalized) { _, apk -> nativeReceipt(apk).toString() }
    }

    @Test fun refusesWrongOriginAndNativeSerializationDisagreement() {
        val wrong = association().getJSONObject("descriptor").put("source", JSONObject().put("origin", "RECORDED"))
        var invoked = false
        assertThrows(IllegalArgumentException::class.java) {
            SourceInputAdmission.attach(JSONObject().put("descriptor", wrong).toString(), "{}", "{}") { _, _ ->
                invoked = true; nativeReceipt("{}").toString()
            }
        }
        assertTrue(!invoked)
        assertThrows(IllegalArgumentException::class.java) {
            SourceInputAdmission.attach(association().toString(), "{}", "{}") { _, _ -> nativeReceipt("different").toString() }
        }
    }

    @Test fun privateOriginalShapeAndTypesRefuseBeforeNativeCall() {
        val fields = listOf(StructuredInputField("selectionSha256", "Selection", required = true, maxLength = 64))
        for (raw in listOf("c".repeat(64), "{}", "{\"selectionSha256\":7}", "{\"selectionSha256\":null}",
            "{\"selectionSha256\":\"value\",\"extra\":\"ignored\"}", " ".repeat(65537))) {
            assertThrows(Exception::class.java) { StructuredInput.normalizeOriginal(fields, raw) }
        }
        assertThrows(IllegalArgumentException::class.java) { StructuredInput.normalizeOriginal(emptyList(), "{}") }
    }
    @Test fun closedStagesDistinguishNativeAdmissionFromAssociationWithoutChangingInput() {
        val stages = mutableListOf<String>()
        val raw = "{\"selectionSha256\":\"${"c".repeat(64)}\"}"
        SourceInputAdmission.attach(association().toString(), raw, raw, { stages.add(it) }) { original, normalized ->
            assertEquals(raw, original); assertEquals(raw, normalized); nativeReceipt(normalized).toString()
        }
        assertEquals(listOf("input-association", "input-native-admission", "input-receipt"), stages)
        stages.clear()
        assertThrows(IllegalArgumentException::class.java) {
            SourceInputAdmission.attach(association().toString(), raw, raw, { stages.add(it) }) { _, _ ->
                throw IllegalArgumentException("private input is never a stage")
            }
        }
        assertEquals(listOf("input-association", "input-native-admission"), stages)
    }

}
