package com.trollm.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionInputTest {
    @Test fun acceptsExactConnectionNamesAndOpaqueTokens() {
        assertTrue(ConnectionInput.validServer("browser-mcp"))
        assertTrue(ConnectionInput.validToken("abc.DEF-123_xyz+/="))
    }

    @Test fun rejectsEmptyWhitespaceAndControlInput() {
        for (input in listOf("", " ", "a b", "a\nb", "a\tb", "a\u0000b", "a\u00a0b")) {
            assertFalse(ConnectionInput.validServer(input))
            assertFalse(ConnectionInput.validToken(input))
        }
        assertFalse(ConnectionInput.validToken("Bearer abc"))
    }
}
