package com.trollm.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Test

class ConnectionInputTest {
    @Test fun incompleteEndpointEditsHaveFieldFeedbackUntilCorrected() {
        for (input in listOf("", "h", "https:", "https://", "https://exa mple.test", "http://example.test/mcp",
            "https://user:secret@example.test/mcp", "https://example.test/mcp#fragment")) {
            val feedback = ConnectionInput.endpointError(input)
            assertNotNull(feedback)
            assertTrue(feedback!!.contains("HTTPS MCP endpoint"))
            assertFalse(feedback.contains("secret"))
            assertFalse(feedback.contains("storage"))
        }
        assertNull(ConnectionInput.endpointError("https://example.test/mcp"))
        assertNotNull(ConnectionInput.endpointError("https://"))
        assertNull(ConnectionInput.endpointError("https://example.test/other?tenant=one"))
    }

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
