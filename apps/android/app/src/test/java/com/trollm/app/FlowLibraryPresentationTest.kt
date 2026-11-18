package com.trollm.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.InstallResult
import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.RunSummaryInfo

class FlowLibraryPresentationTest {
    @Test fun exactPackagedFlowsLeadWhileAnImportWithTheSameIdHasNoStockClaims() {
        val summary = FlowInfo("a", "text-summary", 1u)
        val total = FlowInfo("b", "check-total", 1u)
        val browser = FlowInfo("c", "browser-summary", 1u)
        val imported = FlowInfo("d", summary.id, summary.version)
        val installed = mapOf(
            browser to BundledStarter.BROWSER_SUMMARY,
            total to BundledStarter.CHECK_TOTAL,
            summary to BundledStarter.TEXT_SUMMARY,
        )
        val rows = libraryFlows(listOf(imported, browser, total, summary), emptyMap(), emptyMap(), installed)
        assertEquals(listOf(summary, total, browser, imported), rows.map { it.flow })
        assertEquals("No account needed", rows.first().connectionRequirement)
        assertNull(rows.last().starter)
        assertNull(rows.last().connectionRequirement)
        assertNull(rows.last().modelRequirement)
    }

    @Test fun coldOrdinaryRouteResolvesExactStarterWithoutLibraryAndReusesTheResult() {
        val catalog = StarterFlowCatalog()
        val summary = FlowInfo("summary-pin", "text-summary", 1u)
        val browser = FlowInfo("browser-pin", "browser-summary", 1u)
        val attempted = mutableListOf<BundledStarter>()
        val installAsset: (BundledStarter) -> FlowInfo = { kind ->
            attempted += kind
            when (kind) {
                BundledStarter.TEXT_SUMMARY -> summary
                BundledStarter.CHECK_TOTAL -> error("different manifest already installed")
                BundledStarter.BROWSER_SUMMARY -> browser
            }
        }

        assertNull(catalog.verifiedKind(summary.sha256))
        assertEquals(BundledStarter.TEXT_SUMMARY, catalog.resolveKind(summary.sha256, installAsset))
        assertEquals(BundledStarter.BROWSER_SUMMARY, catalog.verifiedKind(browser.sha256))
        assertNull(catalog.resolveKind("different-pin", installAsset))
        assertEquals(BundledStarter.entries.toList(), attempted)

        // Library opening can retry failures and still offer their diagnostics.
        val installation = catalog.install(installAsset)
        assertEquals("different manifest already installed", installation.errors.single().details)
    }

    @Test fun firstDownloadUsesDistinctEffectiveFilesAndCanReportAnExplicitEmptySetup() {
        val flow = FlowInfo("a", "custom", 1u)
        val pinned = ModelRequirement("Pinned", "https://example.com/pinned.gguf", "a".repeat(64), 532_517_120, 2048)
        val custom = pinned.copy(sha256 = "b".repeat(64), sizeBytes = 3_106_738_272)
        val defaults = FlowSetup("Example", null, null, null, mapOf("coder" to pinned), emptyMap())
        val effective = defaults.copy(models = mapOf("coder" to custom, "reasoner" to custom, "other" to pinned))
        val rows = libraryFlows(listOf(flow), mapOf(flow.sha256 to defaults), mapOf(flow.sha256 to effective), emptyMap())
        assertEquals("2 models · 3.64 GB on first download", rows.single().modelRequirement)
        assertNull(rows.single().connectionRequirement)
        val direct = defaults.copy(models = emptyMap())
        assertEquals("No model files required", LibraryFlow(flow, direct, direct, null).modelRequirement)
        assertNull(LibraryFlow(flow, defaults, null, null).modelRequirement)
    }

    @Test fun importOpensTheIdentifiedFlowRevisionEvenIfAnotherRevisionSharesItsId() {
        val install = InstallResult("manifest", "archive", "flow-pin", "check-total", 2u, 3u, "bundles", "deferred")
        val opened = install.installedFlow()
        assertEquals(FlowInfo("flow-pin", "check-total", 2u), opened)
    }

    @Test fun activeWorkerTakesPriorityOverANewerInterruptionAndTerminalRunsStayInHistory() {
        val summaries = listOf(
            RunSummaryInfo("finished", "flow", null, 400uL, RunPhaseApi.Succeeded, false),
            RunSummaryInfo("interrupted", "flow", null, 300uL, RunPhaseApi.Running, false),
            RunSummaryInfo("paused", "flow", null, 200uL, RunPhaseApi.Paused("user"), true),
            RunSummaryInfo("older", "flow", null, 100uL, RunPhaseApi.Running, true),
        ).associateBy { it.runId }
        val history = historyEntries(summaries.keys.toList(), summaries, emptyMap())
        assertEquals("paused", featuredLibraryRun(history, summaries)?.id)
        val cold = summaries.mapValues { (_, summary) -> summary.copy(workerActive = false) }
        assertEquals("interrupted", featuredLibraryRun(history, cold)?.id)
        assertNull(featuredLibraryRun(history, summaries.filterKeys { it == "finished" }))
    }
}
