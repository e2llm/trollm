package com.trollm.app

import org.junit.Assert.fail
import org.junit.Test

class ModelSetupTest {
    private val valid = ModelRequirement("Coder", "https://example.com/model.gguf", "a".repeat(64), 100, 4096)

    private fun rejects(model: ModelRequirement) {
        try { model.validate(); fail("Expected invalid model to be rejected") }
        catch (_: IllegalArgumentException) { }
    }

    @Test fun acceptsPinnedModelWithExplicitSampling() {
        valid.validate()
        valid.copy(temperature = 0.0, topP = 1.0).validate()
        requireSafeSetupKey("coder-2_custom")
    }

    @Test fun rejectsMissingIdentityInvalidSamplingAndUnsafeUrls() {
        rejects(valid.copy(sha256 = "A".repeat(64)))
        rejects(valid.copy(sizeBytes = 0))
        rejects(valid.copy(ctx = 0))
        rejects(valid.copy(temperature = Double.NaN))
        rejects(valid.copy(temperature = 2.01))
        rejects(valid.copy(topP = 0.0))
        rejects(valid.copy(topP = Double.POSITIVE_INFINITY))
        rejects(valid.copy(url = "http://example.com/model"))
        rejects(valid.copy(url = "https://user:password@example.com/model"))
        rejects(valid.copy(label = " "))
    }

    @Test fun rejectsSlotsThatCouldEscapeLegacyModelDirectory() {
        for (slot in listOf("../coder", "/coder", "coder.gguf", "", "a/b", "a\\b")) {
            try { requireSafeSetupKey(slot); fail("Expected unsafe slot to be rejected") }
            catch (_: IllegalArgumentException) { }
        }
    }
}
