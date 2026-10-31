package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class StructuredInputTest {
    private val fields = listOf(
        StructuredInputField("browser", "Browser", required = true, maxLength = 128),
        StructuredInputField("url", "Page URL", required = true, kind = "url", maxLength = 2048),
        StructuredInputField("question", "Question", maxLength = 2048),
    )

    private fun rejects(block: () -> Unit) {
        try { block(); fail("Expected invalid input to be rejected") } catch (_: Exception) { }
    }

    @Test fun knownFieldsSerializeTrimmedWithJsonEscapingAndEmptyOptionalValues() {
        val values = mapOf("browser" to "  Browser A  ", "url" to " https://example.com/page#section ", "ignored" to "not serialized")
        val json = JSONObject(StructuredInput.serialize(fields, values, "unused"))
        assertEquals(setOf("browser", "url", "question"), json.keys().asSequence().toSet())
        assertEquals("Browser A", json.getString("browser"))
        assertEquals("https://example.com/page#section", json.getString("url"))
        assertEquals("", json.getString("question"))
        val question = "Quote \"this\"\nnext line \\ slash 😀"
        val escaped = JSONObject(StructuredInput.serialize(fields, values + ("question" to question), "unused"))
        assertEquals(question, escaped.getString("question"))
    }

    @Test fun requiredTextAndInvalidUrlsCannotSerialize() {
        val valid = mapOf("browser" to "A", "url" to "https://example.com")
        assertTrue(StructuredInput.errors(fields, valid).isEmpty())
        for (url in listOf("ftp://example.com", "https:///missing-host", "https://user:secret@example.com", "https://@example.com", "/relative", "https://example.com/a b", "https://example.com:999999")) {
            val invalid = valid + ("url" to url)
            assertTrue(StructuredInput.errors(fields, invalid).containsKey("url"))
            rejects { StructuredInput.serialize(fields, invalid, "") }
        }
        assertTrue(StructuredInput.errors(fields, valid + ("url" to "HTTP://example.com/path?q=1")).isEmpty())
        assertTrue(StructuredInput.errors(fields, valid + ("browser" to " \n\u2003")).containsKey("browser"))
        rejects { StructuredInput.serialize(fields, valid - "browser", "") }
    }

    @Test fun lengthLimitsCountUnicodeCodePointsAfterTrimming() {
        val short = listOf(StructuredInputField("text", "Text", maxLength = 2))
        assertTrue(StructuredInput.errors(short, mapOf("text" to "  😀é  ")).isEmpty())
        assertTrue(StructuredInput.errors(short, mapOf("text" to "😀😀😀")).containsKey("text"))
        assertEquals("😀é", JSONObject(StructuredInput.serialize(short, mapOf("text" to " 😀é "), "")).getString("text"))
    }

    @Test fun metadataDefaultsAndFieldsOnlySetupAreSupported() {
        val fieldsOnly = JSONObject("""{"fields":[{"key":"_name2","label":"Name"}]}""")
        val parsed = StructuredInput.fields(fieldsOnly).single()
        assertEquals(StructuredInputField("_name2", "Name"), parsed)
        val setup = FlowSetup.parse("""{"format":"trollm.flow-setup.v1","title":"Browser","models":{},"connections":{},"input":$fieldsOnly}""")
        assertEquals(listOf(parsed), setup.inputFields)
        assertEquals(null, setup.inputLabel)
        assertEquals("", JSONObject(StructuredInput.serialize(setup.inputFields, emptyMap(), "")).getString("_name2"))
    }

    @Test fun nullPlaceholdersAreAbsentForStructuredAndLegacyInputs() {
        val input = JSONObject("""{"label":"Input","placeholder":null,"fields":[{"key":"text","label":"Text","placeholder":null}]}""")
        assertEquals(null, StructuredInput.fields(input).single().placeholder)
        val setup = FlowSetup.parse("""{"format":"trollm.flow-setup.v1","title":"Summary","models":{},"input":$input}""")
        assertEquals(null, setup.inputPlaceholder)
        assertEquals(null, setup.inputFields.single().placeholder)
    }

    @Test fun totalSerializedUtf8BoundIncludesJsonAndEscaping() {
        val many = listOf("a", "b", "c", "d").map { StructuredInputField(it, it, maxLength = 16384) }
        // Each field is independently valid; UTF-8 size can still exceed 64 KiB.
        val unicode = many.associate { it.key to "😀".repeat(16384) }
        assertTrue(StructuredInput.errors(many, unicode).isEmpty())
        rejects { StructuredInput.serialize(many, unicode, "") }
        val emptyBytes = StructuredInput.serialize(many, emptyMap(), "").toByteArray(Charsets.UTF_8).size
        var remaining = StructuredInput.MAX_JSON_BYTES - emptyBytes
        val exact = many.associate { field ->
            val length = minOf(remaining, field.maxLength)
            remaining -= length
            field.key to "x".repeat(length)
        }
        assertEquals(0, remaining)
        assertEquals(65536, StructuredInput.serialize(many, exact, "").toByteArray(Charsets.UTF_8).size)
        rejects { StructuredInput.serialize(many, exact + ("d" to exact.getValue("d") + "x"), "") }
        val escaped = exact + ("a" to "\"".repeat(exact.getValue("a").length))
        assertTrue(StructuredInput.errors(many, escaped).isEmpty())
        rejects { StructuredInput.serialize(many, escaped, "") }
    }

    @Test fun metadataRejectsInvalidOrDuplicateKeysCountsAndLimits() {
        val good = JSONObject("""{"key":"browser","label":"Browser"}""")
        for (bad in listOf(
            JSONObject(good.toString()).put("key", "1name"),
            JSONObject(good.toString()).put("key", "a".repeat(33)),
            JSONObject(good.toString()).put("label", " "),
            JSONObject(good.toString()).put("kind", "number"),
            JSONObject(good.toString()).put("maxLength", 0),
            JSONObject(good.toString()).put("maxLength", 16385),
            JSONObject(good.toString()).put("maxLength", 1.5),
            JSONObject(good.toString()).put("required", "true"),
        )) rejects { StructuredInput.fields(JSONObject().put("fields", org.json.JSONArray().put(bad))) }
        rejects { StructuredInput.fields(JSONObject("""{"fields":[]}""")) }
        rejects { StructuredInput.fields(JSONObject().put("fields", org.json.JSONArray().put(good).put(good))) }
        val tooMany = org.json.JSONArray()
        repeat(9) { tooMany.put(JSONObject(good.toString()).put("key", "name$it")) }
        rejects { StructuredInput.fields(JSONObject().put("fields", tooMany)) }
    }

    @Test fun legacyInputKeepsItsVerbatimTextAndLabelPlaceholder() {
        val legacy = "  supplied text\nwith whitespace  "
        assertTrue(StructuredInput.fields(JSONObject("""{"label":"Text","placeholder":"Paste text"}""")).isEmpty())
        assertEquals(legacy, StructuredInput.serialize(emptyList(), mapOf("ignored" to "value"), legacy))
        val setup = FlowSetup.parse("""{"format":"trollm.flow-setup.v1","title":"Summary","models":{},"input":{"label":"Text","placeholder":"Paste text"}}""")
        assertEquals("Text", setup.inputLabel)
        assertEquals("Paste text", setup.inputPlaceholder)
        assertTrue(setup.inputFields.isEmpty())
        assertFalse(StructuredInput.errors(emptyList(), emptyMap()).isNotEmpty())
    }
    @Test fun originalFormBytesRetainWhitespaceBeforeIndependentNormalization() {
        val declared = listOf(StructuredInputField("text", "Text", required = true))
        val values = mapOf("text" to " \t😀é\n ")
        val raw = StructuredInput.serializeOriginal(declared, values)
        val normalized = StructuredInput.serialize(declared, values, "")
        assertEquals(values.getValue("text"), JSONObject(raw).getString("text"))
        assertEquals("😀é", JSONObject(normalized).getString("text"))
        assertNotEquals(raw, normalized)
        rejects { StructuredInput.serializeOriginal(declared, mapOf("text" to " ".repeat(65536))) }
    }

}
