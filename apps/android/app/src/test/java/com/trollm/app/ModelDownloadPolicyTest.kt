package com.trollm.app

import org.junit.Assert.*
import org.junit.Test

class ModelDownloadPolicyTest {
    private fun model(hash: String, size: Long) = ModelRequirement(
        "Model", "https://example.test/model.gguf", hash.repeat(64), size, 2048,
    )

    @Test fun newPreparationRequiresUnmeteredAndMeteredConsentNeverEnablesRoaming() {
        val defaults = ModelDownloadPolicy()
        assertFalse(defaults.allowMetered)
        assertFalse(defaults.allowRoaming)
        assertTrue(defaults.waitingMessage().contains("unmetered"))
        val consent = ModelDownloadPolicy(allowMetered = true)
        assertTrue(consent.allowMetered)
        assertFalse(consent.allowRoaming)
        assertTrue(consent.description.contains("roaming blocked"))
        // A newly opened preparation has no inherited consent.
        assertFalse(ModelDownloadPolicy().allowMetered)
    }

    @Test fun batchContainsOnlyMissingDistinctAssetsInDeclaredOrder() {
        val models = linkedMapOf(
            "ready" to model("a", 100),
            "ready-alias" to model("a", 100),
            "coder" to model("b", 200),
            "coder-alias" to model("b", 200),
            "reasoner" to model("c", 300),
        )
        val batch = remainingModelDownloads(models, setOf("ready"))
        assertEquals(listOf("coder", "reasoner"), batch.keys.toList())
        assertEquals(500L, batch.values.sumOf { it.sizeBytes })
        assertEquals(listOf("reasoner"), remainingModelDownloads(models, setOf("ready-alias", "coder")).keys.toList())
        assertTrue(remainingModelDownloads(models, setOf("ready", "coder", "reasoner")).isEmpty())
        // Computing a later batch does not expand or mutate the chosen operation.
        assertEquals(listOf("coder", "reasoner"), batch.keys.toList())
    }
}
