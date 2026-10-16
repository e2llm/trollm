package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class FlowSetupOverridesTest {
    private val flow = "a".repeat(64)
    private val otherFlow = "b".repeat(64)
    private val defaults = FlowSetup("Pinned title", "Pinned description", "Input", "Enter text",
        mapOf("coder" to ModelRequirement("Pinned coder", "https://example.com/default.gguf", "c".repeat(64), 100, 4096)),
        mapOf("browser" to ConnectionRequirement("https://example.com/mcp", "oauth", "https://example.com/issuer")))

    private fun fails(block: () -> Unit) {
        try { block(); fail("Expected invalid settings to fail") } catch (_: Exception) { }
    }

    @Test fun persistenceRoundtripIsPerPinnedFlowAndPreservesDescriptorMetadata() {
        val saved = mutableMapOf<String, String>()
        fun store() = FlowSetupOverrideStore({ saved[it] }, { key, value ->
            if (value == null) saved.remove(key) else saved[key] = value
            true
        })
        val customized = defaults.copy(
            models = mapOf("coder" to defaults.models.getValue("coder").copy(url = "https://other.example/model.gguf",
                sha256 = "d".repeat(64), sizeBytes = 200, ctx = 2048, temperature = 0.7, topP = 0.8)),
            connections = mapOf("browser" to defaults.connections.getValue("browser").copy(url = "https://other.example/mcp")),
        )
        store().save(flow, defaults, customized)
        // A newly created store uses persisted bytes, not an in-memory overlay.
        assertEquals(customized, store().load(flow, defaults))
        assertEquals("coder (custom model)", store().load(flow, defaults).modelDisplayLabel("coder", defaults))
        assertEquals(defaults, store().load(otherFlow, defaults))
        val persisted = JSONObject(saved.getValue(flow))
        assertEquals(setOf("version", "models", "connections"), persisted.keys().asSequence().toSet())
        assertFalse(saved.getValue(flow).contains("Pinned title"))
        assertFalse(saved.getValue(flow).contains("issuer"))
        assertFalse(saved.getValue(flow).contains("oauth"))
        store().restore(flow)
        assertTrue(saved.isEmpty())
        assertEquals(defaults, store().load(flow, defaults))
    }

    @Test fun samplingAndEndpointChangesKeepModelIdentityAndPinnedAuth() {
        val changed = defaults.copy(
            title = "Ignored replacement title",
            models = mapOf("coder" to defaults.models.getValue("coder").copy(label = "Ignored label", temperature = 1.0)),
            connections = mapOf("browser" to ConnectionRequirement("https://other.example/mcp", "bearer", null)),
        )
        val applied = FlowSetupOverrides.parse(FlowSetupOverrides.between(defaults, changed).toJson(), defaults).applyTo(defaults)
        assertEquals(defaults.title, applied.title)
        assertEquals(defaults.models.getValue("coder").sha256, applied.models.getValue("coder").sha256)
        assertEquals("Pinned coder", applied.models.getValue("coder").label)
        assertEquals("Pinned coder", applied.modelDisplayLabel("coder", defaults))
        assertEquals(1.0, applied.models.getValue("coder").temperature, 0.0)
        assertEquals("oauth", applied.connections.getValue("browser").auth)
        assertEquals("https://example.com/issuer", applied.connections.getValue("browser").issuerUrl)
    }

    @Test fun staleUnknownAndMalformedStoredEntriesFailWithoutSilentFallback() {
        val valid = FlowSetupOverrides.between(defaults, defaults.copy(models = mapOf("coder" to
            defaults.models.getValue("coder").copy(temperature = 0.5)))).toJson()
        val unknownSlot = JSONObject(valid).also { root ->
            val models = root.getJSONObject("models")
            models.put("missing-slot", models.remove("coder"))
        }.toString()
        val unknownConnection = """{"version":1,"models":{},"connections":{"missing":{"url":"https://example.com/mcp"}}}"""
        val fractional = JSONObject(valid).also { it.getJSONObject("models").getJSONObject("coder").put("ctx", 1.5) }.toString()
        val textSize = JSONObject(valid).also { it.getJSONObject("models").getJSONObject("coder").put("sizeBytes", "100") }.toString()
        for (json in listOf("broken-json", unknownSlot, unknownConnection, fractional, textSize,
            """{"version":2,"models":{},"connections":{}}""",
            """{"version":1,"models":{},"connections":{},"title":"unexpected"}""")) {
            val store = FlowSetupOverrideStore({ json }, { _, _ -> fail("Load must not rewrite invalid settings"); false })
            fails { store.load(flow, defaults) }
        }
    }

    @Test fun invalidOverridesCannotBeSavedAndPersistenceFailuresAreReported() {
        var writes = 0
        val store = FlowSetupOverrideStore({ null }, { _, _ -> writes++; true })
        for (model in listOf(
            defaults.models.getValue("coder").copy(url = "http://example.com/model"),
            defaults.models.getValue("coder").copy(url = "https://user:secret@example.com/model"),
            defaults.models.getValue("coder").copy(url = "https://example.com/model#fragment"),
            defaults.models.getValue("coder").copy(sizeBytes = 0),
            defaults.models.getValue("coder").copy(sha256 = "not-a-hash"),
            defaults.models.getValue("coder").copy(topP = Double.NaN),
        )) fails { store.save(flow, defaults, defaults.copy(models = mapOf("coder" to model))) }
        fails { store.save(flow, defaults, defaults.copy(connections = mapOf("browser" to
            defaults.connections.getValue("browser").copy(url = "https://example.com/mcp#fragment")))) }
        fails { store.save("../flow", defaults, defaults) }
        assertEquals(0, writes)
        val unavailable = FlowSetupOverrideStore({ null }, { _, _ -> false })
        fails { unavailable.save(flow, defaults, defaults) }
        fails { unavailable.restore(flow) }
    }

    @Test fun formDraftRejectsNonNumericAndOutOfRangeValuesWithoutChangingDefaults() {
        val model = defaults.models.getValue("coder")
        val draft = ModelOverrideDraft.from(model)
        assertEquals(model, draft.applyTo(model))
        for (invalid in listOf(draft.copy(sizeBytes = "1.5"), draft.copy(ctx = "2147483648"),
            draft.copy(temperature = "NaN"), draft.copy(topP = "0"), draft.copy(ctx = "0"))) {
            fails { invalid.applyTo(model) }
        }
        assertEquals(4096, defaults.models.getValue("coder").ctx)
    }
}
