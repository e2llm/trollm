package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import uniffi.trollm_uniffi.FlowInfo

class AppNavigationTest {
    private val flow = FlowInfo(sha256 = "a".repeat(64), id = "sample", version = 3u)
    private val run = "1d91cad6-3026-47d2-b790-5427e71e83f1"

    @Test fun connection_round_trip_and_rotation_keep_the_exact_flow_context() {
        val draft = AppNavigation.flow(flow, "draft-1", "installation-pin", "original-association")
        val connection = draft.connections("service", "https://example.com/mcp")
        assertEquals(AppPage.CONNECTIONS, connection.page)
        assertEquals("service", connection.connectionServer)
        val restored = AppNavigation.restore(connection.savedValues())
        assertEquals(draft, restored.back())
    }

    @Test fun starting_preserves_ui_identity_and_source_custody() {
        val draft = AppNavigation.flow(flow, "draft-2", "installation-pin", "original-association")
        val running = draft.started(run)
        assertEquals(draft.runScreenKey, running.runScreenKey)
        assertEquals(draft.flow, running.flow)
        assertEquals(draft.sourceAssociation, running.sourceAssociation)
        assertEquals(run, running.runId)
        assertEquals(running, AppNavigation.restore(running.savedValues()))
        assertEquals(AppNavigation(), running.back())
    }

    @Test fun library_connections_and_saved_runs_do_not_inherit_a_previous_flow() {
        assertEquals(AppNavigation(), AppNavigation().connections().back())
        val selected = AppNavigation.savedRun(run)
        assertEquals(run, selected.runId)
        assertNull(selected.flow)
        assertNull(selected.sourceInstallation)
        assertNull(selected.sourceAssociation)
        assertEquals(selected, AppNavigation.restore(selected.savedValues()))
    }

    @Test fun invalid_saved_state_returns_to_the_library() {
        assertEquals(AppNavigation(), AppNavigation.restore(emptyList()))
        val invalid = AppNavigation.savedRun(run).savedValues().toMutableList()
        invalid[4] = "../not-a-run"
        assertEquals(AppNavigation(), AppNavigation.restore(invalid))
    }

    @Test fun a_durably_selected_uuid_reconciles_an_older_draft_without_resubmission() {
        val draft = AppNavigation.flow(flow, "draft-3")
        assertEquals(AppNavigation.savedRun(run), draft.reconcileSelectedRun(run))
        assertEquals(AppNavigation.savedRun(run), draft.connections().reconcileSelectedRun(run))
        assertEquals(draft, draft.reconcileSelectedRun(null))
        val connection = AppNavigation().connections("service", "https://example.com/mcp")
        assertEquals(connection, connection.reconcileSelectedRun(run))
        assertEquals(AppNavigation.savedRun(run), AppNavigation.savedRun(run).reconcileSelectedRun(null))
    }
}
