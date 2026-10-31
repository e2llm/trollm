package com.trollm.app

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class JsonValuesTest {
    @Test fun platformEnumerationAndNestedComparisonPreserveTypesOrderAndPresence() {
        val first = JSONObject("""{"owner":{"epoch":1,"subject":"original"},"items":[{"id":"a"},null]}""")
        val reordered = JSONObject("""{"items":[{"id":"a"},null],"owner":{"subject":"original","epoch":1}}""")
        assertEquals(setOf("owner", "items"), JsonValues.keys(first))
        assertTrue(JsonValues.same(first, reordered))
        assertFalse(JsonValues.same(first, JSONObject("""{"owner":{"epoch":1,"subject":"other"},"items":[{"id":"a"},null]}""")))
        assertFalse(JsonValues.same(JSONArray("[1,2]"), JSONArray("[2,1]")))
        assertFalse(JsonValues.same(JSONObject("{\"present\":null}"), JSONObject("{}")))
        assertFalse(JsonValues.same(1, "1"))
        assertFalse(JsonValues.same(false, "false"))
        assertFalse(JsonValues.same(1, 1.0))
        assertTrue(JsonValues.same(JSONObject.NULL, JSONObject.NULL))
        try { JSONObject().put("id", 123).sourceString("id"); fail("coerced a wire identity") }
        catch (_: IllegalArgumentException) {}
        assertEquals("original", JSONObject().put("id", "original").sourceString("id"))
        try { JSONArray().put(123).sourceString(0); fail("coerced an evidence digest") }
        catch (_: IllegalArgumentException) {}
    }
}
