package com.trollm.app

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.RunSummaryInfo

class RunHistoryEntriesTest {
    private val old = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    private val recent = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
    private val legacy = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
    private val sha = "d".repeat(64)

    private fun summary(
        id: String,
        time: ULong?,
        phase: RunPhaseApi = RunPhaseApi.Succeeded,
        flowId: String? = "reader",
        flowSha: String? = sha,
    ) = RunSummaryInfo(id, flowId, flowSha, time, phase, false)

    @Test fun newestFirstAndInstalledTitleWithFlowIdAndUuidAvailable() {
        val rows = historyEntries(
            listOf(old, legacy, recent),
            mapOf(old to summary(old, 100uL), recent to summary(recent, 200uL)),
            mapOf(sha to "Web reader"),
        )
        assertEquals(listOf(recent, old, legacy), rows.map { it.id })
        assertEquals("Web reader", rows.first().flowTitle)
        assertEquals("reader", rows.first().flowId)
        assertEquals("Succeeded", rows.first().status)
    }

    @Test fun missingOrCorruptMetadataStillHasAnOpenableUuidRow() {
        val rows = historyEntries(listOf(legacy), emptyMap(), emptyMap())
        assertEquals(legacy, rows.single().id)
        assertEquals("Unknown flow", rows.single().flowTitle)
        assertEquals(null, rows.single().startedAtMs)
        assertEquals("Status unavailable", rows.single().status)
    }

    @Test fun removedFlowUsesSavedIdAndCancellationHasOwnOutcome() {
        val rows = historyEntries(
            listOf(old, recent),
            mapOf(
                old to summary(old, 100uL, RunPhaseApi.Failed("cancelled")),
                recent to summary(recent, 200uL, RunPhaseApi.Running),
            ),
            emptyMap(),
        )
        assertEquals("reader", rows.last().flowTitle)
        assertEquals("Cancelled", rows.last().status)
        assertEquals("Interrupted · Resume", rows.first().status)
    }

    @Test fun overflowingUnsignedTimestampCannotBecomeAnIncorrectDate() {
        val rows = historyEntries(
            listOf(old, recent),
            mapOf(old to summary(old, ULong.MAX_VALUE), recent to summary(recent, 1uL)),
            emptyMap(),
        )
        assertEquals(listOf(recent, old), rows.map { it.id })
        assertEquals(null, rows.last().startedAtMs)
    }
}
