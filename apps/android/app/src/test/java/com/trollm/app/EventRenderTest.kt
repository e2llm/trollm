package com.trollm.app

import com.trollm.app.ui.renderEventJson
import org.junit.Assert.*
import org.junit.Test

class EventRenderTest {
    private fun render(type: String, json: String) = renderEventJson(type, 7uL, json)

    @Test fun absentNullAndMalformedMeasurementsStayUnknown() {
        for (json in listOf("{}", "null", "not JSON", "")) {
            assertEquals("model[?] unknown→unknown tokens, unknown ms", render("model_call", json))
            assertEquals("artifact written (unknown bytes)", render("artifact_written", json))
        }
        for (value in listOf("null", "\"12\"", "\"unknown\"", "true", "[]", "{}", "-1", "-0.5", "0.5", "12.75")) {
            val fields = "{\"tokensIn\":$value,\"tokensOut\":$value,\"ms\":$value,\"bytes\":$value}"
            assertEquals("value $value", "model[?] unknown→unknown tokens, unknown ms", render("model_call", fields))
            assertEquals("value $value", "artifact written (unknown bytes)", render("artifact_written", fields))
        }
    }

    @Test fun invalidMeasurementDoesNotHideOtherRecordedValues() {
        assertEquals("model[coder] unknown→2 tokens, 3 ms",
            render("model_call", """{"slot":"coder","tokensIn":1.5,"tokensOut":2,"ms":3}"""))
        assertEquals("model[coder] 1→unknown tokens, 3 ms",
            render("model_call", """{"slot":"coder","tokensIn":1,"tokensOut":null,"ms":3}"""))
        assertEquals("model[coder] 1→2 tokens, unknown ms",
            render("model_call", """{"slot":"coder","tokensIn":1,"tokensOut":2,"ms":"3"}"""))
    }

    @Test fun actualZeroAndWholeMeasurementsRemainExact() {
        assertEquals("model[coder] 0→0 tokens, 0 ms",
            render("model_call", """{"slot":"coder","tokensIn":0,"tokensOut":0,"ms":0}"""))
        assertEquals("artifact written (0 bytes)", render("artifact_written", """{"bytes":0}"""))
        assertEquals("model[reasoner] 123→45 tokens, 678 ms",
            render("model_call", """{"slot":"reasoner","tokensIn":123,"tokensOut":45,"ms":678}"""))
        assertEquals("artifact written (2048 bytes)", render("artifact_written", """{"bytes":2048}"""))
        // Integral JSON numbers are valid regardless of decimal/exponent spelling.
        assertEquals("model[?] 0→2 tokens, 300 ms",
            render("model_call", """{"tokensIn":0.0,"tokensOut":2.0,"ms":3e2}"""))
    }

    @Test fun measurementsOutsideLongRangeDoNotWrapOrRoundIntoValidValues() {
        assertEquals("artifact written (9223372036854775807 bytes)",
            render("artifact_written", """{"bytes":9223372036854775807}"""))
        for (value in listOf("9223372036854775808", "18446744073709551616", "1e100", "9223372036854775806.5")) {
            assertEquals("value $value", "artifact written (unknown bytes)",
                render("artifact_written", """{"bytes":$value}"""))
        }
    }

    @Test fun finishReasonAppearsOnlyAsRecordedNonblankText() {
        val fields = """"tokensIn":1,"tokensOut":2,"ms":3"""
        for (finish in listOf("null", "\"\"", "\" \"", "false", "1", "{}")) {
            assertEquals("model[?] 1→2 tokens, 3 ms",
                render("model_call", "{$fields,\"finishReason\":$finish}"))
        }
        assertEquals("model[?] 1→2 tokens, 3 ms; finish length",
            render("model_call", "{$fields,\"finishReason\":\"length\"}"))
    }

    @Test fun renderingDoesNotExposeRawResponsesArgumentsOrUnknownEventPayloads() {
        val payload = """{"response":"PRIVATE_RESPONSE","args":{"secret":"PRIVATE_ARG"},"metadata":{"secret":"PRIVATE_METADATA"}}"""
        assertEquals("model[?] unknown→unknown tokens, unknown ms", render("model_call", payload))
        assertEquals("tool intent: browser.act", render("tool_call_intent",
            """{"server":"browser","tool":"act","args":{"secret":"PRIVATE_ARG"}}"""))
        assertEquals("unrecognized (#7)", render("unrecognized", payload))
        assertEquals("sifr_observation (#7)", render("sifr_observation", payload))
    }
}
