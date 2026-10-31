package com.trollm.app

import org.junit.Assert.*
import org.junit.Test

class TechnicalEventsTest {
    @Test fun retainsBoundedRecentLines() {
        var lines = emptyList<String>()
        repeat(100) { batch ->
            lines = TechnicalEvents.append(lines, List(128) { "$batch:$it" })
            assertTrue(lines.size <= TechnicalEvents.MAX_LINES)
        }
        assertEquals("99:127", lines.last())
    }

    @Test fun boundsUtf8BytesAndSkipsOversizedLines() {
        val large = "界".repeat(30000)
        val lines = TechnicalEvents.append(emptyList(), List(10) { large })
        assertTrue(lines.sumOf { it.toByteArray(Charsets.UTF_8).size } <= TechnicalEvents.MAX_BYTES)
        assertEquals(listOf("after"), TechnicalEvents.append(lines, listOf("界".repeat(100000), "after")))
    }
}
