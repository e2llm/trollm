package com.trollm.app

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class EndpointIdentityTest {
    @Test fun matchesSharedRustEndpointIdentityCases() {
        val stream = javaClass.getResourceAsStream("/endpoint-identity.json")
        assertNotNull("Shared endpoint identity resource must be packaged for JVM tests", stream)
        val cases = JSONArray(stream!!.bufferedReader(Charsets.UTF_8).use { it.readText() })
        val ids = mutableSetOf<String>()
        assertTrue("The shared fixture must contain cases", cases.length() > 0)
        repeat(cases.length()) { index ->
            val case = cases.getJSONObject(index)
            val id = case.getString("id")
            assertTrue("Duplicate fixture ID: $id", ids.add(id))
            val input = case.getString("input")
            if (case.isNull("expected")) {
                try {
                    ConnectionInput.canonicalEndpoint(input)
                    fail("$id should be rejected")
                } catch (_: IllegalArgumentException) { }
            } else {
                val expected = case.getString("expected")
                assertEquals(id, expected, ConnectionInput.canonicalEndpoint(input))
                assertEquals("$id canonical output must be stable", expected,
                    ConnectionInput.canonicalEndpoint(expected))
            }
        }
    }

    private fun grants(): OAuthGrants {
        val stored = mutableMapOf<String, StoredConnection>()
        return OAuthGrants(
            { stored[it] ?: StoredConnection(null, null, null) },
            { server, value -> stored[server] = value },
            Any(),
        )
    }

    private val distinctResources = listOf(
        "https://example.test/a/../b" to "https://example.test/b",
        "https://example.test/a/%2e%2e/b" to "https://example.test/b",
        "https://example.test/a/../b" to "https://example.test/a/%2e%2e/b",
        "https://example.test/a%2Fb?q=%2F" to "https://example.test/a/b?q=%2F",
        "https://example.test/a%2Fb?q=%2F" to "https://example.test/a%2Fb?q=/",
        "https://example.test/a%2Fb?q=%2F" to "https://example.test/a%2Fb?q=%2f",
        "https://example.test/mcp?" to "https://example.test/mcp",
    )

    @Test fun manualCredentialsRemainBoundToExactRawPathAndQuery() {
        for ((selected, alias) in distinctResources) {
            // Exercise both directions so neither spelling can substitute the other.
            for ((endpoint, other) in listOf(selected to alias, alias to selected)) {
                val grants = grants()
                grants.selectManual("browser", endpoint, "synthetic-token")
                assertEquals(endpoint, "synthetic-token", grants.manualToken("browser", endpoint))
                assertNull("$endpoint must not authorize $other", grants.manualToken("browser", other))
            }
        }
    }

    @Test fun oauthPinsRemainBoundToExactRawPathAndQuery() {
        for ((selected, alias) in distinctResources) {
            for ((endpoint, other) in listOf(selected to alias, alias to selected)) {
                val grants = grants()
                val login = grants.begin("browser", endpoint, "synthetic-state", "synthetic-pkce")
                assertTrue(grants.update("browser", login.id, login.endpoint, "authorized-state"))
                assertEquals(endpoint, login.id, grants.pin("browser", endpoint)?.grantId)
                assertEquals(endpoint, login.id, grants.bind("browser", endpoint, login.id)?.grantId)
                assertNull("$endpoint must not pin $other", grants.pin("browser", other))
                assertNull("$endpoint must not resume as $other", grants.bind("browser", other, login.id))
            }
        }
    }
}
