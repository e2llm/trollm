package com.trollm.app

import org.junit.Assert.*
import org.junit.Test

class OAuthGrantsTest {
    private class Memory {
        val values = mutableMapOf<String, StoredConnection>()
        private val lock = Any()
        fun open() = OAuthGrants({ values[it] ?: StoredConnection(null, null, null) },
            { name, value -> values[name] = value }, lock)
    }

    private fun grants() = Memory().open()

    @Test fun endpointIdentityPreservesResourcesAndNormalizesOnlyAuthorityDefaults() {
        assertEquals("https://example.test/", ConnectionInput.canonicalEndpoint("HTTPS://EXAMPLE.TEST:443"))
        assertEquals("https://example.test/a%2Fb?q=%2F", ConnectionInput.canonicalEndpoint("https://EXAMPLE.TEST:443/a%2Fb?q=%2F"))
        assertNotEquals(ConnectionInput.canonicalEndpoint("https://example.test/a/../b"),
            ConnectionInput.canonicalEndpoint("https://example.test/b"))
        for (invalid in listOf("http://example.test/mcp", "https://user@example.test/mcp", "https://example.test/#fragment")) {
            try { ConnectionInput.canonicalEndpoint(invalid); fail("invalid endpoint accepted") }
            catch (_: IllegalArgumentException) { }
        }
    }

    @Test fun pendingLoginCannotPinAndCompletedGrantIsEndpointBound() {
        val grants = grants()
        val login = grants.begin("browser", "https://example.test/mcp", "pending-state", "pkce-request")
        assertNull(grants.pin("browser", "https://example.test/mcp"))
        assertTrue(grants.update("browser", login.id, login.endpoint, "authorized-state"))
        assertEquals(login.id, grants.pin("browser", "https://EXAMPLE.TEST:443/mcp")?.grantId)
        assertNull(grants.pin("browser", "https://example.test/other"))
        assertNull(grants.pin("browser", "https://example.test/mcp?tenant=other"))
        assertNull(grants.pin("other", "https://example.test/mcp"))
    }

    @Test fun logoutRejectsLateExchangeAndRefresh() {
        val grants = grants()
        val login = grants.begin("browser", "https://example.test/mcp", "pending-state", "pkce-request")
        grants.logout("browser")
        assertFalse(grants.update("browser", login.id, login.endpoint, "late-secret-state"))
        assertNull(grants.current("browser"))
    }

    @Test fun refreshPersistsRotationWithinTheSameGrant() {
        val grants = grants()
        val login = grants.begin("browser", "https://example.test/mcp", "pending-state", "request")
        assertTrue(grants.update("browser", login.id, login.endpoint, "initial-token-state"))
        assertTrue(grants.update("browser", login.id, login.endpoint, "rotated-token-state"))
        assertEquals(login.id, grants.pin("browser", login.endpoint)?.grantId)
        assertEquals("rotated-token-state", grants.current("browser")?.state)
        assertFalse(grants.update("browser", login.id, "https://example.test/other", "wrong-endpoint"))
        assertEquals("rotated-token-state", grants.current("browser")?.state)
    }

    @Test fun replacementLoginCannotBeOverwrittenOrCancelledByOldCompletion() {
        val grants = grants()
        val first = grants.begin("browser", "https://example.test/mcp", "pending-one", "request-one")
        assertTrue(grants.update("browser", first.id, first.endpoint, "authorized-one"))
        val second = grants.begin("browser", first.endpoint, "pending-two", "request-two")
        assertNotEquals(first.id, second.id)
        assertFalse(grants.update("browser", first.id, first.endpoint, "late-refresh-one"))
        grants.invalidate("browser", first.id)
        assertEquals(second.id, grants.current("browser")?.id)
        assertTrue(grants.update("browser", second.id, second.endpoint, "authorized-two"))
        assertEquals(second.id, grants.pin("browser", first.endpoint)?.grantId)
    }

    @Test fun stateNeverAppearsInGrantStringRepresentation() {
        assertEquals("OAuthGrant(redacted)", OAuthGrant("id", "name", "https://example.test/", "private-state").toString())
        assertEquals("StoredConnection(redacted)", StoredConnection(null, "private-token", null).toString())
    }

    @Test fun legacyMigrationPrefersOAuthThenManualThenAnonymousAndPersistsChoice() {
        val memory = Memory()
        memory.values["oauth"] = StoredConnection(null, "legacy-token",
            OAuthGrant("grant", "oauth", "https://example.test/mcp", "state"))
        memory.values["manual"] = StoredConnection(null, "legacy-token", null)
        val grants = memory.open()
        assertEquals(SelectedAuthMode.OAUTH, grants.mode("oauth"))
        assertEquals(SelectedAuthMode.BEARER, grants.mode("manual"))
        assertEquals(SelectedAuthMode.ANONYMOUS, grants.mode("empty"))
        assertEquals(SelectedAuthMode.OAUTH, memory.values["oauth"]?.mode)
        assertEquals(SelectedAuthMode.BEARER, memory.values["manual"]?.mode)
        assertEquals(SelectedAuthMode.ANONYMOUS, memory.values["empty"]?.mode)
    }

    @Test fun pendingFailedAndLoggedOutOAuthNeverFallsBackToRetainedManualToken() {
        val memory = Memory()
        val grants = memory.open()
        grants.selectManual("browser", "legacy-token")
        val attempt = grants.begin("browser", "https://example.test/mcp", "", "preparing")
        assertEquals(SelectedAuthMode.OAUTH, grants.mode("browser"))
        assertNull(grants.pin("browser", attempt.endpoint))
        grants.invalidate("browser", attempt.id)
        assertEquals("legacy-token", memory.values["browser"]?.manualToken)
        assertEquals(SelectedAuthMode.UNAVAILABLE, memory.open().mode("browser"))
        val next = grants.begin("browser", attempt.endpoint, "", "preparing")
        assertTrue(grants.prepare("browser", next.id, next.endpoint, "state", "request"))
        assertTrue(grants.update("browser", next.id, next.endpoint, "authorized"))
        grants.logout("browser")
        assertEquals(SelectedAuthMode.UNAVAILABLE, memory.open().mode("browser"))
        assertNull(memory.open().bind("browser", next.endpoint, next.id))
    }

    @Test fun explicitManualSaveInvalidatesOAuthAndSelectsManualOrAnonymous() {
        val grants = grants()
        val attempt = grants.begin("browser", "https://example.test/mcp", "", "preparing")
        grants.selectManual("browser", "chosen-token")
        assertEquals(SelectedAuthMode.BEARER, grants.mode("browser"))
        assertNull(grants.current("browser"))
        assertFalse(grants.prepare("browser", attempt.id, attempt.endpoint, "late-state", "request"))
        assertFalse(grants.update("browser", attempt.id, attempt.endpoint, "late-token-state"))
        grants.invalidate("browser", attempt.id)
        assertEquals(SelectedAuthMode.BEARER, grants.mode("browser"))
        grants.selectManual("browser", null)
        assertEquals(SelectedAuthMode.ANONYMOUS, grants.mode("browser"))
    }

    @Test fun coldBindingRequiresSavedGrantAndEndpointWithoutChoosingReplacement() {
        val memory = Memory()
        val initial = memory.open()
        val first = initial.begin("browser", "https://example.test/mcp", "state", "request")
        assertTrue(initial.update("browser", first.id, first.endpoint, "authorized"))
        val cold = memory.open()
        assertEquals(first.id, cold.bind("browser", "https://EXAMPLE.TEST:443/mcp", first.id)?.grantId)
        assertNull(cold.bind("browser", "https://example.test/other", first.id))
        assertNull(cold.bind("other", first.endpoint, first.id))
        val replacement = initial.begin("browser", first.endpoint, "new-state", "request")
        assertTrue(initial.update("browser", replacement.id, first.endpoint, "new-authorized"))
        assertNull(cold.bind("browser", first.endpoint, first.id))
        assertEquals(replacement.id, cold.current("browser")?.id)
        assertEquals(replacement.id, cold.bind("browser", first.endpoint, replacement.id)?.grantId)
    }
}
