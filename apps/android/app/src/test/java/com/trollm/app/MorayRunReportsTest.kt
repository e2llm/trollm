package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

class MorayRunReportsTest {
    private val run = "abc123ab-1234-4234-8234-123456789abc"
    @Test fun retainThenReopenAndRetryDoesNotPrepareOrRepeatExecution() {
        val directory = Files.createTempDirectory("native-report-ui").toFile()
        try {
            var prepared = 0; var retained = 0; var sends = 0
            val actual = JSONObject().put("source", JSONObject().put("nativeRunId", run)).put("report", JSONObject()
                .put("reportId", "native-original").put("runId", run).put("outcome", "unknown"))
            val first = MorayRunReports(directory,
                { prepared++; actual },
                { id, value -> assertEquals(run, id); assertTrue(JsonValues.same(value, actual)); retained++; "native-original" },
                { _, _ -> sends++; throw java.io.IOException("reply lost") })
            first.retain(run)
            assertEquals(0, sends)
            try { first.send(run); fail() } catch (_: java.io.IOException) {}
            val reopened = MorayRunReports(directory,
                { fail("retry prepared new work"); actual },
                { _, _ -> fail("retry retained a different report"); "" },
                { value, id -> assertEquals("native-original", id); assertTrue(JsonValues.same(value, actual)); sends++
                    JSONObject().put("verification", "device-reported") })
            assertEquals("unknown", reopened.retained(run)!!.getJSONObject("native").getJSONObject("report").getString("outcome"))
            val sent = reopened.send(run)
            assertTrue(sent.has("receipt"))
            reopened.send(run) // retained acknowledgment, not a new delivery or run
            assertEquals(1, prepared); assertEquals(1, retained); assertEquals(2, sends)
        } finally { directory.deleteRecursively() }
    }
    @Test fun missingOutboxCannotSendAndMalformedRunCannotSelectAnotherFile() {
        val directory = Files.createTempDirectory("native-report-refusal").toFile()
        try {
            val controller = MorayRunReports(directory,
                { fail(); JSONObject() }, { _, _ -> fail(); "" }, { _, _ -> fail(); JSONObject() })
            try { controller.send(run); fail() } catch (_: MorayFailure) {}
            try { controller.retained("../other"); fail() } catch (_: IllegalArgumentException) {}
        } finally { directory.deleteRecursively() }
    }
}
